use super::*;
use crate::restrictions::{
    ProtocolModuleId, RestrictionMode, RestrictionRecord, RestrictionStatus, RestrictionTarget,
    GUARDIAN_RESTRICTION_MAX_SLOTS,
};
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
struct IncidentGuardianPauseTarget {
    symbol: String,
    pause_function: String,
}

static INCIDENT_GUARDIAN_PAUSE_TARGETS: OnceLock<Vec<IncidentGuardianPauseTarget>> =
    OnceLock::new();

fn incident_guardian_pause_targets() -> &'static [IncidentGuardianPauseTarget] {
    INCIDENT_GUARDIAN_PAUSE_TARGETS
        .get_or_init(|| {
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/incident-guardian-pause-allowlist.json"
            )))
            .expect("incident guardian pause allowlist must be valid JSON")
        })
        .as_slice()
}

fn is_allowlisted_incident_guardian_pause(symbol: &str, function: &str) -> bool {
    incident_guardian_pause_targets()
        .iter()
        .any(|target| target.symbol == symbol && target.pause_function == function)
}

fn is_bridge_committee_admin_contract_call(symbol: &str, function: &str) -> bool {
    (symbol.eq_ignore_ascii_case("BRIDGE") || symbol.eq_ignore_ascii_case("LICHENBRIDGE"))
        && matches!(
            function,
            "add_bridge_validator"
                | "remove_bridge_validator"
                | "set_required_confirmations"
                | "set_request_timeout"
        )
}

fn is_oracle_committee_admin_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    if ((symbol.eq_ignore_ascii_case("ORACLE") || symbol.eq_ignore_ascii_case("LICHENORACLE"))
        && matches!(function, "add_price_feeder" | "set_authorized_attester"))
        || (matches!(symbol, "LUSD" | "WSOL" | "WETH" | "WBNB") && function == "set_attester")
    {
        return true;
    }

    if symbol.eq_ignore_ascii_case("DEXMARGIN") || symbol.eq_ignore_ascii_case("MARGIN") {
        return matches!(function, "set_mark_price" | "set_index_price")
            || (function == "call" && matches!(args.first().copied(), Some(1) | Some(31)));
    }

    false
}

fn is_immediate_oracle_committee_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    (symbol.eq_ignore_ascii_case("DEXMARGIN") || symbol.eq_ignore_ascii_case("MARGIN"))
        && (matches!(function, "set_mark_price" | "set_index_price")
            || (function == "call" && matches!(args.first().copied(), Some(1) | Some(31))))
}

fn is_treasury_executor_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    if symbol.eq_ignore_ascii_case("DEXMARGIN") {
        return function == "call" && args.first() == Some(&9u8);
    }

    if symbol.eq_ignore_ascii_case("DEXAMM") || symbol.eq_ignore_ascii_case("AMM") {
        return function == "call" && args.first() == Some(&21u8);
    }

    ((symbol.eq_ignore_ascii_case("LEND") || symbol.eq_ignore_ascii_case("THALLLEND"))
        && function == "withdraw_reserves")
        || ((symbol.eq_ignore_ascii_case("SPOREVAULT") || symbol.eq_ignore_ascii_case("VAULT"))
            && function == "withdraw_protocol_fees")
        || (symbol.eq_ignore_ascii_case("SPOREPUMP") && function == "withdraw_fees")
        || (matches!(symbol, "LUSD" | "WSOL" | "WETH" | "WBNB") && function == "set_minter")
}

fn guardian_protocol_module_allowed(module: ProtocolModuleId) -> bool {
    matches!(
        module,
        ProtocolModuleId::Bridge
            | ProtocolModuleId::Contracts
            | ProtocolModuleId::Custody
            | ProtocolModuleId::Dex
            | ProtocolModuleId::Lending
            | ProtocolModuleId::Marketplace
            | ProtocolModuleId::Oracle
            | ProtocolModuleId::Shielded
            | ProtocolModuleId::Tokens
    )
}

fn guardian_restrict_target_mode_allowed(
    target: &RestrictionTarget,
    mode: &RestrictionMode,
) -> bool {
    match (target, mode) {
        (RestrictionTarget::Account(_), RestrictionMode::OutgoingOnly) => true,
        (
            RestrictionTarget::Contract(_),
            RestrictionMode::StateChangingBlocked | RestrictionMode::Quarantined,
        ) => true,
        (RestrictionTarget::CodeHash(_), RestrictionMode::DeployBlocked) => true,
        (RestrictionTarget::BridgeRoute { .. }, RestrictionMode::RoutePaused) => true,
        (RestrictionTarget::ProtocolModule(module), RestrictionMode::ProtocolPaused) => {
            guardian_protocol_module_allowed(*module)
        }
        _ => false,
    }
}

fn guardian_ttl_cap_after(slot: u64) -> Result<u64, String> {
    slot.checked_add(GUARDIAN_RESTRICTION_MAX_SLOTS)
        .ok_or_else(|| "Incident guardian restriction TTL cap overflows slot range".to_string())
}

fn validate_guardian_expiry(
    current_slot: u64,
    expires_at_slot: Option<u64>,
    context: &str,
) -> Result<u64, String> {
    let expires_at_slot = expires_at_slot.ok_or_else(|| {
        format!(
            "{} must include expires_at_slot and cannot be indefinite",
            context
        )
    })?;
    if expires_at_slot <= current_slot {
        return Err(format!(
            "{} expiry {} must be after current slot {}",
            context, expires_at_slot, current_slot
        ));
    }

    let max_expires_at_slot = guardian_ttl_cap_after(current_slot)?;
    if expires_at_slot > max_expires_at_slot {
        return Err(format!(
            "{} expiry {} exceeds guardian TTL cap {} from current slot {}",
            context, expires_at_slot, GUARDIAN_RESTRICTION_MAX_SLOTS, current_slot
        ));
    }

    Ok(expires_at_slot)
}

impl TxProcessor {
    pub(super) fn governance_action_uses_immediate_risk_reduction_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(
            is_allowlisted_incident_guardian_pause(entry.symbol.as_str(), function.as_str())
                || is_immediate_oracle_committee_contract_call(
                    entry.symbol.as_str(),
                    function.as_str(),
                    args.as_slice(),
                ),
        )
    }

    pub(super) fn governance_action_requires_treasury_executor_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        if matches!(action, GovernanceAction::TreasuryTransfer { .. }) {
            return Ok(true);
        }

        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_treasury_executor_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
            args.as_slice(),
        ))
    }

    pub(super) fn governance_action_requires_upgrade_proposer_policy(
        &self,
        action: &GovernanceAction,
    ) -> bool {
        matches!(
            action,
            GovernanceAction::ContractUpgrade { .. }
                | GovernanceAction::SetContractUpgradeTimelock { .. }
                | GovernanceAction::ExecuteContractUpgrade { .. }
        )
    }

    pub(super) fn governance_action_requires_upgrade_veto_guardian_policy(
        &self,
        action: &GovernanceAction,
    ) -> bool {
        matches!(action, GovernanceAction::VetoContractUpgrade { .. })
    }

    pub(super) fn governance_action_requires_bridge_committee_admin_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract, function, ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_bridge_committee_admin_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
        ))
    }

    pub(super) fn governance_action_requires_oracle_committee_admin_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_oracle_committee_admin_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
            args.as_slice(),
        ))
    }

    pub(super) fn guardian_extension_chain_info(
        &self,
        record: &RestrictionRecord,
    ) -> Result<(u32, u64), String> {
        let mut extension_count = 0u32;
        let mut root_created_slot = record.created_slot;
        let mut cursor = record.supersedes;

        while let Some(previous_id) = cursor {
            extension_count = extension_count.checked_add(1).ok_or_else(|| {
                format!(
                    "Restriction {} guardian extension chain is too deep",
                    record.id
                )
            })?;
            if extension_count > 64 {
                return Err(format!(
                    "Restriction {} guardian extension chain is too deep",
                    record.id
                ));
            }

            let previous = self.b_get_restriction(previous_id)?.ok_or_else(|| {
                format!(
                    "Restriction {} supersedes missing restriction {}",
                    record.id, previous_id
                )
            })?;
            root_created_slot = previous.created_slot;
            cursor = previous.supersedes;
        }

        Ok((extension_count, root_created_slot))
    }

    pub(super) fn validate_guardian_owned_temporary_record(
        &self,
        record: &RestrictionRecord,
        guardian_authority: &Pubkey,
        current_slot: u64,
    ) -> Result<(), String> {
        if record.approval_authority != Some(*guardian_authority) {
            return Err(format!(
                "Incident guardian may only lift or extend restrictions it created; restriction {} has different approval authority",
                record.id
            ));
        }
        if record.effective_status(current_slot) != RestrictionStatus::Active {
            return Err(format!(
                "Incident guardian may only manage active restrictions; restriction {} is {} at slot {}",
                record.id,
                record.effective_status(current_slot).as_str(),
                current_slot
            ));
        }
        if record.expires_at_slot.is_none() {
            return Err(format!(
                "Incident guardian may only manage temporary restrictions; restriction {} is indefinite",
                record.id
            ));
        }

        Ok(())
    }

    pub(super) fn validate_incident_guardian_restriction_action(
        &self,
        action: &GovernanceAction,
        guardian_authority: &Pubkey,
    ) -> Result<(), String> {
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        match action {
            GovernanceAction::Restrict {
                target,
                mode,
                expires_at_slot,
                ..
            } => {
                if !guardian_restrict_target_mode_allowed(target, mode) {
                    return Err(format!(
                        "Incident guardian restriction target/mode is not allowed: target_type={} mode={}",
                        target.target_type_label(),
                        mode.as_str()
                    ));
                }
                validate_guardian_expiry(
                    current_slot,
                    *expires_at_slot,
                    "Incident guardian restriction",
                )?;
            }
            GovernanceAction::LiftRestriction { restriction_id, .. } => {
                if *restriction_id == 0 {
                    return Err(
                        "Incident guardian LiftRestriction restriction_id must be greater than zero"
                            .to_string(),
                    );
                }
                let record = self
                    .b_get_restriction(*restriction_id)?
                    .ok_or_else(|| format!("Restriction {} not found", restriction_id))?;
                self.validate_guardian_owned_temporary_record(
                    &record,
                    guardian_authority,
                    current_slot,
                )?;
            }
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                ..
            } => {
                if *restriction_id == 0 {
                    return Err(
                        "Incident guardian ExtendRestriction restriction_id must be greater than zero"
                            .to_string(),
                    );
                }
                let record = self
                    .b_get_restriction(*restriction_id)?
                    .ok_or_else(|| format!("Restriction {} not found", restriction_id))?;
                self.validate_guardian_owned_temporary_record(
                    &record,
                    guardian_authority,
                    current_slot,
                )?;

                let (extension_count, root_created_slot) =
                    self.guardian_extension_chain_info(&record)?;
                if extension_count >= 1 {
                    return Err(format!(
                        "Incident guardian may extend restriction {} only once",
                        restriction_id
                    ));
                }

                let new_expires_at_slot = validate_guardian_expiry(
                    current_slot,
                    *new_expires_at_slot,
                    "Incident guardian restriction extension",
                )?;
                let current_expires_at_slot = record.expires_at_slot.ok_or_else(|| {
                    format!(
                        "Incident guardian restriction {} has no expiry to extend",
                        restriction_id
                    )
                })?;
                if new_expires_at_slot <= current_expires_at_slot {
                    return Err(format!(
                        "Incident guardian restriction {} new expiry {} must be greater than current expiry {}",
                        restriction_id, new_expires_at_slot, current_expires_at_slot
                    ));
                }

                let cumulative_duration = GUARDIAN_RESTRICTION_MAX_SLOTS
                    .checked_mul(2)
                    .ok_or_else(|| {
                        "Incident guardian cumulative TTL cap overflows slot range".to_string()
                    })?;
                let cumulative_cap = root_created_slot
                    .checked_add(cumulative_duration)
                    .ok_or_else(|| {
                        "Incident guardian cumulative TTL cap overflows slot range".to_string()
                    })?;
                if new_expires_at_slot > cumulative_cap {
                    return Err(format!(
                        "Incident guardian restriction {} expiry {} exceeds cumulative TTL cap {} from root slot {}",
                        restriction_id, new_expires_at_slot, cumulative_duration, root_created_slot
                    ));
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub(super) fn validate_governance_proposal_restriction_policy(
        &self,
        proposal: &GovernanceProposal,
    ) -> Result<(), String> {
        let Some((guardian_authority, _)) = self.get_governed_incident_guardian_authority()? else {
            return Ok(());
        };
        if proposal.approval_authority == Some(guardian_authority) {
            self.validate_incident_guardian_restriction_action(
                &proposal.action,
                &guardian_authority,
            )?;
        }
        Ok(())
    }
}
