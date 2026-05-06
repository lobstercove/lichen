use super::*;
use crate::restrictions::{
    ProtocolModuleId, RestrictionLiftReason, RestrictionMode, RestrictionReason, RestrictionTarget,
    MAX_BRIDGE_ROUTE_COMPONENT_LEN,
};

const RESTRICT_FLAG_EVIDENCE_HASH: u8 = 0x01;
const RESTRICT_FLAG_EVIDENCE_URI_HASH: u8 = 0x02;
const RESTRICT_FLAG_EXPIRES_AT_SLOT: u8 = 0x04;
const RESTRICT_FLAGS_ALLOWED: u8 =
    RESTRICT_FLAG_EVIDENCE_HASH | RESTRICT_FLAG_EVIDENCE_URI_HASH | RESTRICT_FLAG_EXPIRES_AT_SLOT;

const EXTEND_FLAG_EXPIRES_AT_SLOT: u8 = 0x01;
const EXTEND_FLAG_EVIDENCE_HASH: u8 = 0x02;
const EXTEND_FLAGS_ALLOWED: u8 = EXTEND_FLAG_EXPIRES_AT_SLOT | EXTEND_FLAG_EVIDENCE_HASH;

struct GovernanceActionReader<'a> {
    action: &'static str,
    data: &'a [u8],
    offset: usize,
}

impl<'a> GovernanceActionReader<'a> {
    fn new(action: &'static str, data: &'a [u8]) -> Self {
        Self {
            action,
            data,
            offset: 0,
        }
    }

    fn read_exact(&mut self, len: usize, field: &str) -> Result<&'a [u8], String> {
        if self.data.len().saturating_sub(self.offset) < len {
            return Err(format!("{} payload truncated at {}", self.action, field));
        }
        let start = self.offset;
        self.offset += len;
        Ok(&self.data[start..start + len])
    }

    fn read_u8(&mut self, field: &str) -> Result<u8, String> {
        Ok(self.read_exact(1, field)?[0])
    }

    fn read_u16_le(&mut self, field: &str) -> Result<u16, String> {
        let bytes = self.read_exact(2, field)?;
        Ok(u16::from_le_bytes(bytes.try_into().map_err(|_| {
            format!("{} invalid u16 encoding for {}", self.action, field)
        })?))
    }

    fn read_u64_le(&mut self, field: &str) -> Result<u64, String> {
        let bytes = self.read_exact(8, field)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            format!("{} invalid u64 encoding for {}", self.action, field)
        })?))
    }

    fn read_pubkey(&mut self, field: &str) -> Result<Pubkey, String> {
        let bytes = self.read_exact(32, field)?;
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(bytes);
        Ok(Pubkey(pubkey))
    }

    fn read_hash(&mut self, field: &str) -> Result<Hash, String> {
        let bytes = self.read_exact(32, field)?;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(bytes);
        Ok(Hash(hash))
    }

    fn read_limited_utf8(&mut self, field: &str) -> Result<String, String> {
        let len = self.read_u16_le(&format!("{}_len", field))? as usize;
        if len == 0 {
            return Err(format!("{} {} cannot be empty", self.action, field));
        }
        if len > MAX_BRIDGE_ROUTE_COMPONENT_LEN {
            return Err(format!(
                "{} {} length {} exceeds {}",
                self.action, field, len, MAX_BRIDGE_ROUTE_COMPONENT_LEN
            ));
        }
        let bytes = self.read_exact(len, field)?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_string())
            .map_err(|_| format!("{} {} must be valid UTF-8", self.action, field))
    }

    fn finish(&self) -> Result<(), String> {
        if self.offset != self.data.len() {
            return Err(format!(
                "{} payload has {} trailing bytes",
                self.action,
                self.data.len() - self.offset
            ));
        }
        Ok(())
    }
}

fn parse_restriction_target(
    reader: &mut GovernanceActionReader<'_>,
) -> Result<RestrictionTarget, String> {
    let target_type = reader.read_u8("target_type")?;
    let target = match target_type {
        0 => RestrictionTarget::Account(reader.read_pubkey("account")?),
        1 => RestrictionTarget::AccountAsset {
            account: reader.read_pubkey("account")?,
            asset: reader.read_pubkey("asset")?,
        },
        2 => RestrictionTarget::Asset(reader.read_pubkey("asset")?),
        3 => RestrictionTarget::Contract(reader.read_pubkey("contract")?),
        4 => RestrictionTarget::CodeHash(reader.read_hash("code_hash")?),
        5 => RestrictionTarget::BridgeRoute {
            chain_id: reader.read_limited_utf8("chain_id")?,
            asset: reader.read_limited_utf8("asset")?,
        },
        6 => {
            let module_id = reader.read_u8("module_id")?;
            RestrictionTarget::ProtocolModule(ProtocolModuleId::from_u8(module_id).ok_or_else(
                || format!("{} unknown protocol module id {}", reader.action, module_id),
            )?)
        }
        _ => {
            return Err(format!(
                "{} unknown restriction target type {}",
                reader.action, target_type
            ));
        }
    };
    target
        .validate()
        .map_err(|e| format!("{} {}", reader.action, e))?;
    Ok(target)
}

fn parse_restriction_mode(
    reader: &mut GovernanceActionReader<'_>,
) -> Result<RestrictionMode, String> {
    let mode_id = reader.read_u8("restriction_mode")?;
    let frozen_amount = if mode_id == 3 {
        Some(reader.read_u64_le("frozen_amount")?)
    } else {
        None
    };
    let mode = RestrictionMode::from_u8(mode_id, frozen_amount)
        .ok_or_else(|| format!("{} unknown restriction mode {}", reader.action, mode_id))?;
    mode.validate()
        .map_err(|e| format!("{} {}", reader.action, e))?;
    Ok(mode)
}

fn parse_restriction_reason(
    reader: &mut GovernanceActionReader<'_>,
) -> Result<RestrictionReason, String> {
    let reason_id = reader.read_u8("restriction_reason")?;
    RestrictionReason::from_u8(reason_id)
        .ok_or_else(|| format!("{} unknown restriction reason {}", reader.action, reason_id))
}

impl TxProcessor {
    pub(super) fn parse_symbol_registration_fields(
        &self,
        json_bytes: &[u8],
    ) -> Result<SymbolRegistrationSpec, String> {
        let raw = std::str::from_utf8(json_bytes)
            .map_err(|_| "RegisterSymbol: invalid UTF-8 data".to_string())?;
        let payload: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| format!("RegisterSymbol: invalid JSON: {}", e))?;

        let symbol = payload
            .get("symbol")
            .and_then(|s| s.as_str())
            .ok_or_else(|| "RegisterSymbol: missing 'symbol' field".to_string())?
            .to_string();
        validate_symbol_registry_field_length("symbol", &symbol, MAX_SYMBOL_REGISTRY_SYMBOL_LEN)?;

        let name = payload
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.to_string());
        if let Some(ref value) = name {
            validate_symbol_registry_field_length("name", value, MAX_SYMBOL_REGISTRY_NAME_LEN)?;
        }

        let template = payload
            .get("template")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        if let Some(ref value) = template {
            validate_symbol_registry_field_length(
                "template",
                value,
                MAX_SYMBOL_REGISTRY_TEMPLATE_LEN,
            )?;
        }

        Ok(SymbolRegistrationSpec {
            symbol,
            name,
            template,
            metadata: payload.get("metadata").cloned(),
            decimals: payload
                .get("decimals")
                .and_then(|d| d.as_u64())
                .map(|d| d as u8),
        })
    }

    fn projected_fee_config_for_governance_change(
        &self,
        param_id: u8,
        value: u64,
    ) -> Result<FeeConfig, String> {
        let mut fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let pending_changes = {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(batch) = guard.as_ref() {
                batch.get_pending_governance_changes()?
            } else {
                self.state.get_pending_governance_changes()?
            }
        };

        for (pending_param_id, pending_value) in pending_changes {
            fee_config.apply_governance_param(pending_param_id, pending_value);
        }

        fee_config.apply_governance_param(param_id, value);
        Ok(fee_config)
    }

    fn is_fee_distribution_param(param_id: u8) -> bool {
        matches!(
            param_id,
            GOV_PARAM_FEE_BURN_PERCENT
                | GOV_PARAM_FEE_PRODUCER_PERCENT
                | GOV_PARAM_FEE_VOTERS_PERCENT
                | GOV_PARAM_FEE_TREASURY_PERCENT
                | GOV_PARAM_FEE_COMMUNITY_PERCENT
        )
    }

    pub(super) fn validate_governance_param_change_value(
        &self,
        param_id: u8,
        value: u64,
    ) -> Result<(), String> {
        match param_id {
            GOV_PARAM_BASE_FEE => {
                if value == 0 || value > 1_000_000_000 {
                    return Err(
                        "GovernanceParamChange: base_fee must be 1..=1_000_000_000 spores"
                            .to_string(),
                    );
                }
            }
            GOV_PARAM_FEE_BURN_PERCENT
            | GOV_PARAM_FEE_PRODUCER_PERCENT
            | GOV_PARAM_FEE_VOTERS_PERCENT
            | GOV_PARAM_FEE_TREASURY_PERCENT
            | GOV_PARAM_FEE_COMMUNITY_PERCENT => {
                if value > 100 {
                    return Err("GovernanceParamChange: fee percentage must be 0..=100".to_string());
                }
            }
            GOV_PARAM_MIN_VALIDATOR_STAKE => {
                if !(1_000_000_000..=1_000_000_000_000_000_000).contains(&value) {
                    return Err(
                        "GovernanceParamChange: min_validator_stake out of range".to_string()
                    );
                }
            }
            GOV_PARAM_EPOCH_SLOTS => {
                if !(1_000..=10_000_000).contains(&value) {
                    return Err(
                        "GovernanceParamChange: epoch_slots must be 1_000..=10_000_000".to_string(),
                    );
                }
            }
            _ => {
                return Err(format!(
                    "GovernanceParamChange: unknown param_id {}",
                    param_id
                ));
            }
        }

        Ok(())
    }

    fn validate_governance_param_change(&self, param_id: u8, value: u64) -> Result<(), String> {
        self.validate_governance_param_change_value(param_id, value)?;

        if Self::is_fee_distribution_param(param_id) {
            self.projected_fee_config_for_governance_change(param_id, value)?
                .validate_distribution()
                .map_err(|e| format!("GovernanceParamChange: {}", e))?;
        }

        Ok(())
    }

    fn parse_restrict_governance_action(&self, data: &[u8]) -> Result<GovernanceAction, String> {
        let mut reader = GovernanceActionReader::new("Restrict", data);
        let target = parse_restriction_target(&mut reader)?;
        let mode = parse_restriction_mode(&mut reader)?;
        let reason = parse_restriction_reason(&mut reader)?;
        let flags = reader.read_u8("flags")?;
        if flags & !RESTRICT_FLAGS_ALLOWED != 0 {
            return Err(format!("Restrict unexpected flags 0x{:02x}", flags));
        }

        let evidence_hash = if flags & RESTRICT_FLAG_EVIDENCE_HASH != 0 {
            Some(reader.read_hash("evidence_hash")?)
        } else {
            None
        };
        let evidence_uri_hash = if flags & RESTRICT_FLAG_EVIDENCE_URI_HASH != 0 {
            Some(reader.read_hash("evidence_uri_hash")?)
        } else {
            None
        };
        let expires_at_slot = if flags & RESTRICT_FLAG_EXPIRES_AT_SLOT != 0 {
            Some(reader.read_u64_le("expires_at_slot")?)
        } else {
            None
        };
        reader.finish()?;

        if reason.requires_evidence() && evidence_hash.is_none() && evidence_uri_hash.is_none() {
            return Err(format!(
                "Restrict reason {} requires evidence_hash or evidence_uri_hash",
                reason.as_str()
            ));
        }

        Ok(GovernanceAction::Restrict {
            target,
            mode,
            reason,
            evidence_hash,
            evidence_uri_hash,
            expires_at_slot,
        })
    }

    fn parse_lift_restriction_governance_action(
        &self,
        data: &[u8],
    ) -> Result<GovernanceAction, String> {
        let mut reader = GovernanceActionReader::new("LiftRestriction", data);
        let restriction_id = reader.read_u64_le("restriction_id")?;
        if restriction_id == 0 {
            return Err("LiftRestriction restriction_id must be greater than zero".to_string());
        }
        let reason_id = reader.read_u8("lift_reason")?;
        let reason = RestrictionLiftReason::from_u8(reason_id).ok_or_else(|| {
            format!(
                "LiftRestriction unknown restriction lift reason {}",
                reason_id
            )
        })?;
        reader.finish()?;

        Ok(GovernanceAction::LiftRestriction {
            restriction_id,
            reason,
        })
    }

    fn parse_extend_restriction_governance_action(
        &self,
        data: &[u8],
    ) -> Result<GovernanceAction, String> {
        let mut reader = GovernanceActionReader::new("ExtendRestriction", data);
        let restriction_id = reader.read_u64_le("restriction_id")?;
        if restriction_id == 0 {
            return Err("ExtendRestriction restriction_id must be greater than zero".to_string());
        }
        let flags = reader.read_u8("flags")?;
        if flags & !EXTEND_FLAGS_ALLOWED != 0 {
            return Err(format!(
                "ExtendRestriction unexpected flags 0x{:02x}",
                flags
            ));
        }

        let new_expires_at_slot = if flags & EXTEND_FLAG_EXPIRES_AT_SLOT != 0 {
            Some(reader.read_u64_le("new_expires_at_slot")?)
        } else {
            None
        };
        let evidence_hash = if flags & EXTEND_FLAG_EVIDENCE_HASH != 0 {
            Some(reader.read_hash("evidence_hash")?)
        } else {
            None
        };
        reader.finish()?;

        Ok(GovernanceAction::ExtendRestriction {
            restriction_id,
            new_expires_at_slot,
            evidence_hash,
        })
    }

    pub(super) fn tx_updates_governance_fee_distribution(tx: &Transaction) -> bool {
        tx.message.instructions.iter().any(|instruction| {
            instruction.program_id == SYSTEM_PROGRAM_ID
                && instruction.data.len() >= 2
                && instruction.data[0] == 29
                && Self::is_fee_distribution_param(instruction.data[1])
        })
    }

    pub(super) fn validate_pending_governance_fee_distribution(&self) -> Result<(), String> {
        let mut fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let pending_changes = {
            let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(batch) = guard.as_ref() {
                batch.get_pending_governance_changes()?
            } else {
                self.state.get_pending_governance_changes()?
            }
        };

        for (param_id, value) in pending_changes {
            fee_config.apply_governance_param(param_id, value);
        }

        fee_config
            .validate_distribution()
            .map_err(|e| format!("GovernanceParamChange: {}", e))
    }

    pub(super) fn parse_governance_action(
        &self,
        ix: &Instruction,
    ) -> Result<(Pubkey, Pubkey, GovernanceAction), String> {
        if ix.accounts.len() < 2 {
            return Err("Governance action requires [proposer, governance_authority]".to_string());
        }
        if ix.data.len() < 2 {
            return Err("Governance action missing action type".to_string());
        }

        let proposer = ix.accounts[0];
        let authority_account = ix.accounts[1];

        let action = match ix.data[1] {
            GOVERNANCE_ACTION_TREASURY_TRANSFER => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "TreasuryTransfer requires [proposer, governance_authority, recipient]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 10 {
                    return Err("TreasuryTransfer missing amount".to_string());
                }
                let amount = u64::from_le_bytes(
                    ix.data[2..10]
                        .try_into()
                        .map_err(|_| "Invalid treasury transfer amount encoding".to_string())?,
                );
                if amount == 0 {
                    return Err("TreasuryTransfer amount must be > 0".to_string());
                }
                GovernanceAction::TreasuryTransfer {
                    recipient: ix.accounts[2],
                    amount,
                }
            }
            GOVERNANCE_ACTION_PARAM_CHANGE => {
                if ix.data.len() < 11 {
                    return Err("GovernanceParamChange missing param_id/value".to_string());
                }
                let param_id = ix.data[2];
                let value = u64::from_le_bytes(
                    ix.data[3..11]
                        .try_into()
                        .map_err(|_| "Invalid governance param value encoding".to_string())?,
                );
                self.validate_governance_param_change(param_id, value)?;
                GovernanceAction::ParamChange { param_id, value }
            }
            GOVERNANCE_ACTION_CONTRACT_UPGRADE => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "ContractUpgrade requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 6 {
                    return Err("ContractUpgrade missing code length".to_string());
                }
                let code_len = u32::from_le_bytes(
                    ix.data[2..6]
                        .try_into()
                        .map_err(|_| "Invalid contract upgrade length encoding".to_string())?,
                ) as usize;
                if ix.data.len() < 6 + code_len {
                    return Err("ContractUpgrade code payload truncated".to_string());
                }
                GovernanceAction::ContractUpgrade {
                    contract: ix.accounts[2],
                    code: ix.data[6..6 + code_len].to_vec(),
                }
            }
            GOVERNANCE_ACTION_SET_UPGRADE_TIMELOCK => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "SetContractUpgradeTimelock requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 6 {
                    return Err("SetContractUpgradeTimelock missing epochs".to_string());
                }
                let epochs = u32::from_le_bytes(
                    ix.data[2..6]
                        .try_into()
                        .map_err(|_| "Invalid timelock epoch encoding".to_string())?,
                );
                GovernanceAction::SetContractUpgradeTimelock {
                    contract: ix.accounts[2],
                    epochs,
                }
            }
            GOVERNANCE_ACTION_EXECUTE_UPGRADE => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "ExecuteContractUpgrade requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                GovernanceAction::ExecuteContractUpgrade {
                    contract: ix.accounts[2],
                }
            }
            GOVERNANCE_ACTION_VETO_UPGRADE => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "VetoContractUpgrade requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                GovernanceAction::VetoContractUpgrade {
                    contract: ix.accounts[2],
                }
            }
            GOVERNANCE_ACTION_CONTRACT_CLOSE => {
                if ix.accounts.len() < 4 {
                    return Err(
                        "ContractClose requires [proposer, governance_authority, contract, destination]"
                            .to_string(),
                    );
                }
                GovernanceAction::ContractClose {
                    contract: ix.accounts[2],
                    destination: ix.accounts[3],
                }
            }
            GOVERNANCE_ACTION_CONTRACT_CALL => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "ContractCall requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 16 {
                    return Err("ContractCall missing payload".to_string());
                }

                let value = u64::from_le_bytes(
                    ix.data[2..10]
                        .try_into()
                        .map_err(|_| "Invalid contract call value encoding".to_string())?,
                );
                let function_len =
                    u16::from_le_bytes(ix.data[10..12].try_into().map_err(|_| {
                        "Invalid contract call function length encoding".to_string()
                    })?) as usize;
                if function_len == 0 {
                    return Err("ContractCall function name cannot be empty".to_string());
                }
                let args_len_offset = 12 + function_len;
                if ix.data.len() < args_len_offset + 4 {
                    return Err("ContractCall function payload truncated".to_string());
                }
                let function = std::str::from_utf8(&ix.data[12..args_len_offset])
                    .map_err(|_| "ContractCall function name must be valid UTF-8".to_string())?
                    .to_string();
                let args_len = u32::from_le_bytes(
                    ix.data[args_len_offset..args_len_offset + 4]
                        .try_into()
                        .map_err(|_| "Invalid contract call args length encoding".to_string())?,
                ) as usize;
                let args_offset = args_len_offset + 4;
                if ix.data.len() < args_offset + args_len {
                    return Err("ContractCall args payload truncated".to_string());
                }

                GovernanceAction::ContractCall {
                    contract: ix.accounts[2],
                    function,
                    args: ix.data[args_offset..args_offset + args_len].to_vec(),
                    value,
                }
            }
            GOVERNANCE_ACTION_REGISTER_SYMBOL => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "RegisterContractSymbol requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 6 {
                    return Err("RegisterContractSymbol missing payload length".to_string());
                }
                let payload_len =
                    u32::from_le_bytes(ix.data[2..6].try_into().map_err(|_| {
                        "Invalid register symbol payload length encoding".to_string()
                    })?) as usize;
                if ix.data.len() < 6 + payload_len {
                    return Err("RegisterContractSymbol payload truncated".to_string());
                }
                let registration =
                    self.parse_symbol_registration_fields(&ix.data[6..6 + payload_len])?;
                GovernanceAction::RegisterContractSymbol {
                    contract: ix.accounts[2],
                    symbol: registration.symbol,
                    name: registration.name,
                    template: registration.template,
                    metadata: registration.metadata,
                    decimals: registration.decimals,
                }
            }
            GOVERNANCE_ACTION_SET_CONTRACT_ABI => {
                if ix.accounts.len() < 3 {
                    return Err(
                        "SetContractAbi requires [proposer, governance_authority, contract]"
                            .to_string(),
                    );
                }
                if ix.data.len() < 6 {
                    return Err("SetContractAbi missing ABI length".to_string());
                }
                let abi_len = u32::from_le_bytes(
                    ix.data[2..6]
                        .try_into()
                        .map_err(|_| "Invalid ABI length encoding".to_string())?,
                ) as usize;
                if ix.data.len() < 6 + abi_len {
                    return Err("SetContractAbi payload truncated".to_string());
                }
                let abi: ContractAbi = serde_json::from_slice(&ix.data[6..6 + abi_len])
                    .map_err(|e| format!("Invalid ABI format: {}", e))?;
                GovernanceAction::SetContractAbi {
                    contract: ix.accounts[2],
                    abi,
                }
            }
            GOVERNANCE_ACTION_RESTRICT => self.parse_restrict_governance_action(&ix.data[2..])?,
            GOVERNANCE_ACTION_LIFT_RESTRICTION => {
                self.parse_lift_restriction_governance_action(&ix.data[2..])?
            }
            GOVERNANCE_ACTION_EXTEND_RESTRICTION => {
                self.parse_extend_restriction_governance_action(&ix.data[2..])?
            }
            action_type => {
                return Err(format!("Unknown governance action type {}", action_type));
            }
        };

        Ok((proposer, authority_account, action))
    }
}
