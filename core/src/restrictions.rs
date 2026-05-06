use crate::{Hash, Pubkey};
use serde::{Deserialize, Serialize};

pub const GUARDIAN_RESTRICTION_MAX_SLOTS: u64 = 648_000;
pub const MAX_BRIDGE_ROUTE_COMPONENT_LEN: usize = 256;

pub const NATIVE_LICN_ASSET_ID: Pubkey = Pubkey([
    0x4c, 0x49, 0x43, 0x4e, 0x5f, 0x4e, 0x41, 0x54, 0x49, 0x56, 0x45, 0x5f, 0x41, 0x53, 0x53, 0x45,
    0x54, 0x5f, 0x49, 0x44, 0x5f, 0x56, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProtocolModuleId {
    Native = 0,
    Governance = 1,
    Staking = 2,
    MossStake = 3,
    Shielded = 4,
    Contracts = 5,
    Tokens = 6,
    Dex = 7,
    Lending = 8,
    Marketplace = 9,
    Bridge = 10,
    Custody = 11,
    Oracle = 12,
    Validator = 13,
    Mempool = 14,
}

impl ProtocolModuleId {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Native),
            1 => Some(Self::Governance),
            2 => Some(Self::Staking),
            3 => Some(Self::MossStake),
            4 => Some(Self::Shielded),
            5 => Some(Self::Contracts),
            6 => Some(Self::Tokens),
            7 => Some(Self::Dex),
            8 => Some(Self::Lending),
            9 => Some(Self::Marketplace),
            10 => Some(Self::Bridge),
            11 => Some(Self::Custody),
            12 => Some(Self::Oracle),
            13 => Some(Self::Validator),
            14 => Some(Self::Mempool),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Governance => "governance",
            Self::Staking => "staking",
            Self::MossStake => "moss_stake",
            Self::Shielded => "shielded",
            Self::Contracts => "contracts",
            Self::Tokens => "tokens",
            Self::Dex => "dex",
            Self::Lending => "lending",
            Self::Marketplace => "marketplace",
            Self::Bridge => "bridge",
            Self::Custody => "custody",
            Self::Oracle => "oracle",
            Self::Validator => "validator",
            Self::Mempool => "mempool",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RestrictionTarget {
    Account(Pubkey),
    AccountAsset { account: Pubkey, asset: Pubkey },
    Asset(Pubkey),
    Contract(Pubkey),
    CodeHash(Hash),
    BridgeRoute { chain_id: String, asset: String },
    ProtocolModule(ProtocolModuleId),
}

impl RestrictionTarget {
    pub fn target_type_id(&self) -> u8 {
        match self {
            Self::Account(_) => 0,
            Self::AccountAsset { .. } => 1,
            Self::Asset(_) => 2,
            Self::Contract(_) => 3,
            Self::CodeHash(_) => 4,
            Self::BridgeRoute { .. } => 5,
            Self::ProtocolModule(_) => 6,
        }
    }

    pub fn target_type_label(&self) -> &'static str {
        match self {
            Self::Account(_) => "account",
            Self::AccountAsset { .. } => "account_asset",
            Self::Asset(_) => "asset",
            Self::Contract(_) => "contract",
            Self::CodeHash(_) => "code_hash",
            Self::BridgeRoute { .. } => "bridge_route",
            Self::ProtocolModule(_) => "protocol_module",
        }
    }

    pub fn canonical_key(&self) -> Vec<u8> {
        let mut key = Vec::new();
        key.push(self.target_type_id());
        match self {
            Self::Account(account) | Self::Asset(account) | Self::Contract(account) => {
                key.extend_from_slice(&account.0);
            }
            Self::AccountAsset { account, asset } => {
                key.extend_from_slice(&account.0);
                key.extend_from_slice(&asset.0);
            }
            Self::CodeHash(hash) => {
                key.extend_from_slice(&hash.0);
            }
            Self::BridgeRoute { chain_id, asset } => {
                key.extend_from_slice(&(chain_id.len() as u16).to_be_bytes());
                key.extend_from_slice(chain_id.as_bytes());
                key.extend_from_slice(&(asset.len() as u16).to_be_bytes());
                key.extend_from_slice(asset.as_bytes());
            }
            Self::ProtocolModule(module) => {
                key.push(module.as_u8());
            }
        }
        key
    }

    pub fn target_value_label(&self) -> String {
        match self {
            Self::Account(account) | Self::Asset(account) | Self::Contract(account) => {
                account.to_base58()
            }
            Self::AccountAsset { account, asset } => {
                format!("{}:{}", account.to_base58(), asset.to_base58())
            }
            Self::CodeHash(hash) => hash.to_hex(),
            Self::BridgeRoute { chain_id, asset } => format!("{}:{}", chain_id, asset),
            Self::ProtocolModule(module) => module.as_str().to_string(),
        }
    }

    pub fn code_hash(&self) -> Option<Hash> {
        match self {
            Self::CodeHash(hash) => Some(*hash),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::BridgeRoute { chain_id, asset } => {
                if chain_id.is_empty() {
                    return Err("Bridge route chain_id cannot be empty".to_string());
                }
                if asset.is_empty() {
                    return Err("Bridge route asset cannot be empty".to_string());
                }
                if chain_id.len() > MAX_BRIDGE_ROUTE_COMPONENT_LEN {
                    return Err(format!(
                        "Bridge route chain_id length {} exceeds {}",
                        chain_id.len(),
                        MAX_BRIDGE_ROUTE_COMPONENT_LEN
                    ));
                }
                if asset.len() > MAX_BRIDGE_ROUTE_COMPONENT_LEN {
                    return Err(format!(
                        "Bridge route asset length {} exceeds {}",
                        asset.len(),
                        MAX_BRIDGE_ROUTE_COMPONENT_LEN
                    ));
                }
            }
            Self::ProtocolModule(_) => {}
            Self::Account(_)
            | Self::AccountAsset { .. }
            | Self::Asset(_)
            | Self::Contract(_)
            | Self::CodeHash(_) => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RestrictionMode {
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

impl RestrictionMode {
    pub fn from_u8(value: u8, frozen_amount: Option<u64>) -> Option<Self> {
        match value {
            0 => Some(Self::OutgoingOnly),
            1 => Some(Self::IncomingOnly),
            2 => Some(Self::Bidirectional),
            3 => frozen_amount.map(|amount| Self::FrozenAmount { amount }),
            4 => Some(Self::AssetPaused),
            5 => Some(Self::ExecuteBlocked),
            6 => Some(Self::StateChangingBlocked),
            7 => Some(Self::Quarantined),
            8 => Some(Self::DeployBlocked),
            9 => Some(Self::RoutePaused),
            10 => Some(Self::ProtocolPaused),
            11 => Some(Self::Terminated),
            _ => None,
        }
    }

    pub fn mode_id(&self) -> u8 {
        match self {
            Self::OutgoingOnly => 0,
            Self::IncomingOnly => 1,
            Self::Bidirectional => 2,
            Self::FrozenAmount { .. } => 3,
            Self::AssetPaused => 4,
            Self::ExecuteBlocked => 5,
            Self::StateChangingBlocked => 6,
            Self::Quarantined => 7,
            Self::DeployBlocked => 8,
            Self::RoutePaused => 9,
            Self::ProtocolPaused => 10,
            Self::Terminated => 11,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OutgoingOnly => "outgoing_only",
            Self::IncomingOnly => "incoming_only",
            Self::Bidirectional => "bidirectional",
            Self::FrozenAmount { .. } => "frozen_amount",
            Self::AssetPaused => "asset_paused",
            Self::ExecuteBlocked => "execute_blocked",
            Self::StateChangingBlocked => "state_changing_blocked",
            Self::Quarantined => "quarantined",
            Self::DeployBlocked => "deploy_blocked",
            Self::RoutePaused => "route_paused",
            Self::ProtocolPaused => "protocol_paused",
            Self::Terminated => "terminated",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Self::FrozenAmount { amount } = self {
            if *amount == 0 {
                return Err("FrozenAmount restriction amount must be > 0".to_string());
            }
        }
        Ok(())
    }

    pub fn blocks_outgoing(&self) -> bool {
        matches!(self, Self::OutgoingOnly | Self::Bidirectional)
    }

    pub fn blocks_incoming(&self) -> bool {
        matches!(self, Self::IncomingOnly | Self::Bidirectional)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RestrictionStatus {
    Active,
    Expired,
    Lifted,
    Superseded,
}

impl RestrictionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Lifted => "lifted",
            Self::Superseded => "superseded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RestrictionReason {
    ExploitActive = 0,
    StolenFunds = 1,
    BridgeCompromise = 2,
    OracleCompromise = 3,
    ScamContract = 4,
    MaliciousCodeHash = 5,
    SanctionsOrLegalOrder = 6,
    PhishingOrImpersonation = 7,
    CustodyIncident = 8,
    ProtocolBug = 9,
    GovernanceErrorCorrection = 10,
    FalsePositiveLift = 11,
    TestnetDrill = 12,
}

impl RestrictionReason {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::ExploitActive),
            1 => Some(Self::StolenFunds),
            2 => Some(Self::BridgeCompromise),
            3 => Some(Self::OracleCompromise),
            4 => Some(Self::ScamContract),
            5 => Some(Self::MaliciousCodeHash),
            6 => Some(Self::SanctionsOrLegalOrder),
            7 => Some(Self::PhishingOrImpersonation),
            8 => Some(Self::CustodyIncident),
            9 => Some(Self::ProtocolBug),
            10 => Some(Self::GovernanceErrorCorrection),
            11 => Some(Self::FalsePositiveLift),
            12 => Some(Self::TestnetDrill),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExploitActive => "exploit_active",
            Self::StolenFunds => "stolen_funds",
            Self::BridgeCompromise => "bridge_compromise",
            Self::OracleCompromise => "oracle_compromise",
            Self::ScamContract => "scam_contract",
            Self::MaliciousCodeHash => "malicious_code_hash",
            Self::SanctionsOrLegalOrder => "sanctions_or_legal_order",
            Self::PhishingOrImpersonation => "phishing_or_impersonation",
            Self::CustodyIncident => "custody_incident",
            Self::ProtocolBug => "protocol_bug",
            Self::GovernanceErrorCorrection => "governance_error_correction",
            Self::FalsePositiveLift => "false_positive_lift",
            Self::TestnetDrill => "testnet_drill",
        }
    }

    pub fn requires_evidence(self) -> bool {
        !matches!(self, Self::TestnetDrill)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RestrictionLiftReason {
    IncidentResolved = 0,
    FalsePositive = 1,
    EvidenceRejected = 2,
    GovernanceOverride = 3,
    ExpiredCleanup = 4,
    TestnetDrillComplete = 5,
}

impl RestrictionLiftReason {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::IncidentResolved),
            1 => Some(Self::FalsePositive),
            2 => Some(Self::EvidenceRejected),
            3 => Some(Self::GovernanceOverride),
            4 => Some(Self::ExpiredCleanup),
            5 => Some(Self::TestnetDrillComplete),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::IncidentResolved => "incident_resolved",
            Self::FalsePositive => "false_positive",
            Self::EvidenceRejected => "evidence_rejected",
            Self::GovernanceOverride => "governance_override",
            Self::ExpiredCleanup => "expired_cleanup",
            Self::TestnetDrillComplete => "testnet_drill_complete",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestrictionRecord {
    pub id: u64,
    pub target: RestrictionTarget,
    pub mode: RestrictionMode,
    pub status: RestrictionStatus,
    pub reason: RestrictionReason,
    pub evidence_hash: Option<Hash>,
    pub evidence_uri_hash: Option<Hash>,
    pub proposer: Pubkey,
    pub authority: Pubkey,
    pub approval_authority: Option<Pubkey>,
    pub created_slot: u64,
    pub created_epoch: u64,
    pub expires_at_slot: Option<u64>,
    pub supersedes: Option<u64>,
    pub lifted_by: Option<Pubkey>,
    pub lifted_slot: Option<u64>,
    pub lift_reason: Option<RestrictionLiftReason>,
}

impl RestrictionRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("Restriction ID must be greater than zero".to_string());
        }
        self.target.validate()?;
        self.mode.validate()?;
        validate_target_mode(&self.target, &self.mode)?;

        if self.reason.requires_evidence()
            && self.evidence_hash.is_none()
            && self.evidence_uri_hash.is_none()
        {
            return Err(format!(
                "Restriction reason {} requires evidence_hash or evidence_uri_hash",
                self.reason.as_str()
            ));
        }

        if let Some(expires_at_slot) = self.expires_at_slot {
            if expires_at_slot <= self.created_slot {
                return Err("Restriction expiry must be after created_slot".to_string());
            }
        }

        match self.status {
            RestrictionStatus::Active
            | RestrictionStatus::Expired
            | RestrictionStatus::Superseded => {
                if self.lifted_by.is_some()
                    || self.lifted_slot.is_some()
                    || self.lift_reason.is_some()
                {
                    return Err(format!(
                        "{} restriction cannot include lift metadata",
                        self.status.as_str()
                    ));
                }
            }
            RestrictionStatus::Lifted => {
                if self.lifted_by.is_none()
                    || self.lifted_slot.is_none()
                    || self.lift_reason.is_none()
                {
                    return Err(
                        "Lifted restriction requires lifted_by, lifted_slot, and lift_reason"
                            .to_string(),
                    );
                }
                if self.lifted_slot.unwrap_or(0) < self.created_slot {
                    return Err("Lifted restriction slot cannot precede created_slot".to_string());
                }
            }
        }

        Ok(())
    }

    pub fn effective_status(&self, slot: u64) -> RestrictionStatus {
        if self.status == RestrictionStatus::Active {
            if let Some(expires_at_slot) = self.expires_at_slot {
                if slot >= expires_at_slot {
                    return RestrictionStatus::Expired;
                }
            }
        }
        self.status
    }

    pub fn is_effectively_active(&self, slot: u64) -> bool {
        self.effective_status(slot) == RestrictionStatus::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EffectiveRestrictionRecord {
    pub record: RestrictionRecord,
    pub effective_status: RestrictionStatus,
}

impl EffectiveRestrictionRecord {
    pub fn new(record: RestrictionRecord, slot: u64) -> Self {
        let effective_status = record.effective_status(slot);
        Self {
            record,
            effective_status,
        }
    }

    pub fn is_active(&self) -> bool {
        self.effective_status == RestrictionStatus::Active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestrictionTransferDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractRestrictionAccess {
    ReadOnly,
    StateChanging,
}

pub fn transferable_after_frozen_amount(spendable: u64, frozen_amount: u64) -> u64 {
    spendable.saturating_sub(frozen_amount)
}

pub fn restriction_mode_blocks_transfer(
    mode: &RestrictionMode,
    direction: RestrictionTransferDirection,
    amount: u64,
    spendable: u64,
) -> bool {
    match (mode, direction) {
        (RestrictionMode::OutgoingOnly, RestrictionTransferDirection::Outgoing)
        | (RestrictionMode::IncomingOnly, RestrictionTransferDirection::Incoming)
        | (RestrictionMode::Bidirectional, _) => true,
        (
            RestrictionMode::FrozenAmount {
                amount: frozen_amount,
            },
            RestrictionTransferDirection::Outgoing,
        ) => amount > transferable_after_frozen_amount(spendable, *frozen_amount),
        _ => false,
    }
}

fn validate_target_mode(target: &RestrictionTarget, mode: &RestrictionMode) -> Result<(), String> {
    let valid = match target {
        RestrictionTarget::Account(_) => matches!(
            mode,
            RestrictionMode::OutgoingOnly
                | RestrictionMode::IncomingOnly
                | RestrictionMode::Bidirectional
        ),
        RestrictionTarget::AccountAsset { .. } => matches!(
            mode,
            RestrictionMode::OutgoingOnly
                | RestrictionMode::IncomingOnly
                | RestrictionMode::Bidirectional
                | RestrictionMode::FrozenAmount { .. }
        ),
        RestrictionTarget::Asset(_) => matches!(mode, RestrictionMode::AssetPaused),
        RestrictionTarget::Contract(_) => matches!(
            mode,
            RestrictionMode::ExecuteBlocked
                | RestrictionMode::StateChangingBlocked
                | RestrictionMode::Quarantined
                | RestrictionMode::Terminated
        ),
        RestrictionTarget::CodeHash(_) => matches!(mode, RestrictionMode::DeployBlocked),
        RestrictionTarget::BridgeRoute { .. } => matches!(mode, RestrictionMode::RoutePaused),
        RestrictionTarget::ProtocolModule(_) => matches!(mode, RestrictionMode::ProtocolPaused),
    };

    if valid {
        Ok(())
    } else {
        Err(format!(
            "Restriction mode {} is not valid for target type {}",
            mode.as_str(),
            target.target_type_label()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey([byte; 32])
    }

    fn evidence(byte: u8) -> Hash {
        Hash([byte; 32])
    }

    fn base_record() -> RestrictionRecord {
        RestrictionRecord {
            id: 1,
            target: RestrictionTarget::Account(pk(1)),
            mode: RestrictionMode::OutgoingOnly,
            status: RestrictionStatus::Active,
            reason: RestrictionReason::StolenFunds,
            evidence_hash: Some(evidence(9)),
            evidence_uri_hash: None,
            proposer: pk(2),
            authority: pk(3),
            approval_authority: None,
            created_slot: 10,
            created_epoch: 1,
            expires_at_slot: Some(20),
            supersedes: None,
            lifted_by: None,
            lifted_slot: None,
            lift_reason: None,
        }
    }

    #[test]
    fn restriction_record_validation_accepts_valid_record() {
        base_record().validate().unwrap();
    }

    #[test]
    fn restriction_record_validation_rejects_missing_evidence() {
        let mut record = base_record();
        record.evidence_hash = None;
        let err = record.validate().unwrap_err();
        assert!(err.contains("requires evidence_hash"));

        record.reason = RestrictionReason::TestnetDrill;
        record.validate().unwrap();
    }

    #[test]
    fn restriction_record_validation_rejects_invalid_target_mode() {
        let mut record = base_record();
        record.target = RestrictionTarget::CodeHash(evidence(5));
        let err = record.validate().unwrap_err();
        assert!(err.contains("not valid for target type code_hash"));
    }

    #[test]
    fn effective_status_expires_without_mutating_record() {
        let record = base_record();
        assert_eq!(record.effective_status(19), RestrictionStatus::Active);
        assert_eq!(record.effective_status(20), RestrictionStatus::Expired);
        assert_eq!(record.status, RestrictionStatus::Active);
    }

    #[test]
    fn canonical_target_key_distinguishes_target_forms() {
        let account = RestrictionTarget::Account(pk(1));
        let asset = RestrictionTarget::Asset(pk(1));
        let account_asset = RestrictionTarget::AccountAsset {
            account: pk(1),
            asset: NATIVE_LICN_ASSET_ID,
        };

        assert_ne!(account.canonical_key(), asset.canonical_key());
        assert_ne!(account.canonical_key(), account_asset.canonical_key());
    }

    #[test]
    fn frozen_amount_uses_saturating_transferable_floor() {
        assert_eq!(transferable_after_frozen_amount(100, 40), 60);
        assert_eq!(transferable_after_frozen_amount(100, 150), 0);
    }

    #[test]
    fn transferable_after_frozen_amount_preserves_partial_and_future_freeze() {
        assert_eq!(transferable_after_frozen_amount(1_000, 250), 750);
        assert_eq!(transferable_after_frozen_amount(1_000, 1_000), 0);
        assert_eq!(transferable_after_frozen_amount(1_000, 1_200), 0);
        assert_eq!(transferable_after_frozen_amount(1_500, 1_200), 300);
    }
}
