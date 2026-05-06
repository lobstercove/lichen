use super::*;
use crate::restrictions::{
    RestrictionLiftReason, RestrictionMode, RestrictionReason, RestrictionRecord,
    RestrictionStatus, RestrictionTarget,
};
use std::collections::HashMap;

struct RestrictGovernanceAction {
    target: RestrictionTarget,
    mode: RestrictionMode,
    reason: RestrictionReason,
    evidence_hash: Option<Hash>,
    evidence_uri_hash: Option<Hash>,
    expires_at_slot: Option<u64>,
}

impl TxProcessor {
    fn governance_proposal_event_data(
        proposal: &GovernanceProposal,
        actor: &Pubkey,
    ) -> HashMap<String, String> {
        let mut data = HashMap::new();
        data.insert("proposal_id".to_string(), proposal.id.to_string());
        data.insert("action".to_string(), proposal.action_label.clone());
        data.insert("authority".to_string(), proposal.authority.to_base58());
        data.insert("proposer".to_string(), proposal.proposer.to_base58());
        data.insert("actor".to_string(), actor.to_base58());
        data.insert(
            "approvals".to_string(),
            proposal.approvals.len().to_string(),
        );
        data.insert("threshold".to_string(), proposal.threshold.to_string());
        data.insert(
            "execute_after_epoch".to_string(),
            proposal.execute_after_epoch.to_string(),
        );
        data.insert(
            "velocity_tier".to_string(),
            proposal.velocity_tier.as_str().to_string(),
        );
        data.insert(
            "daily_cap_spores".to_string(),
            proposal.daily_cap_spores.to_string(),
        );
        if let Some(approval_authority) = proposal.approval_authority {
            data.insert(
                "approval_authority".to_string(),
                approval_authority.to_base58(),
            );
        }
        data.insert("executed".to_string(), proposal.executed.to_string());
        data.insert("cancelled".to_string(), proposal.cancelled.to_string());
        data.insert("metadata".to_string(), proposal.metadata.clone());
        data
    }

    pub(super) fn emit_governance_proposal_event(
        &self,
        event_name: &str,
        proposal: &GovernanceProposal,
        actor: &Pubkey,
    ) -> Result<(), String> {
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut data = Self::governance_proposal_event_data(proposal, actor);
        for (key, value) in proposal.action.event_fields() {
            data.insert(key.to_string(), value);
        }

        let event = ContractEvent {
            program: SYSTEM_PROGRAM_ID,
            name: event_name.to_string(),
            data,
            slot: current_slot,
        };

        self.b_put_contract_event(&SYSTEM_PROGRAM_ID, &event)
    }

    fn add_restriction_record_event_fields(
        data: &mut HashMap<String, String>,
        record: &RestrictionRecord,
    ) {
        data.insert("restriction_id".to_string(), record.id.to_string());
        data.insert(
            "restriction_status".to_string(),
            record.status.as_str().to_string(),
        );
        data.insert(
            "restriction_target_type".to_string(),
            record.target.target_type_label().to_string(),
        );
        data.insert(
            "restriction_target".to_string(),
            record.target.target_value_label(),
        );
        data.insert(
            "restriction_mode".to_string(),
            record.mode.as_str().to_string(),
        );
        if let RestrictionMode::FrozenAmount { amount } = record.mode {
            data.insert("restriction_amount".to_string(), amount.to_string());
        }
        data.insert(
            "restriction_reason".to_string(),
            record.reason.as_str().to_string(),
        );
        data.insert("created_slot".to_string(), record.created_slot.to_string());
        data.insert(
            "created_epoch".to_string(),
            record.created_epoch.to_string(),
        );
        if let Some(expires_at_slot) = record.expires_at_slot {
            data.insert("expires_at_slot".to_string(), expires_at_slot.to_string());
        }
        if let Some(evidence_hash) = record.evidence_hash {
            data.insert("evidence_hash".to_string(), evidence_hash.to_hex());
        }
        if let Some(evidence_uri_hash) = record.evidence_uri_hash {
            data.insert("evidence_uri_hash".to_string(), evidence_uri_hash.to_hex());
        }
        if let Some(supersedes) = record.supersedes {
            data.insert("supersedes".to_string(), supersedes.to_string());
        }
        if let Some(lifted_by) = record.lifted_by {
            data.insert("lifted_by".to_string(), lifted_by.to_base58());
        }
        if let Some(lifted_slot) = record.lifted_slot {
            data.insert("lifted_slot".to_string(), lifted_slot.to_string());
        }
        if let Some(lift_reason) = record.lift_reason {
            data.insert("lift_reason".to_string(), lift_reason.as_str().to_string());
        }
    }

    fn emit_restriction_lifecycle_event(
        &self,
        event_name: &str,
        proposal: &GovernanceProposal,
        actor: &Pubkey,
        record: &RestrictionRecord,
    ) -> Result<(), String> {
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut data = Self::governance_proposal_event_data(proposal, actor);
        Self::add_restriction_record_event_fields(&mut data, record);

        let event = ContractEvent {
            program: SYSTEM_PROGRAM_ID,
            name: event_name.to_string(),
            data,
            slot: current_slot,
        };

        self.b_put_contract_event(&SYSTEM_PROGRAM_ID, &event)
    }

    fn validate_contract_lifecycle_restriction_action(
        &self,
        target: &RestrictionTarget,
        mode: &RestrictionMode,
        expires_at_slot: Option<u64>,
    ) -> Result<(), String> {
        let RestrictionTarget::Contract(contract_address) = target else {
            return Ok(());
        };
        if contract_lifecycle_status_for_restriction_mode(mode).is_none() {
            return Ok(());
        }
        if matches!(mode, RestrictionMode::Terminated) && expires_at_slot.is_some() {
            return Err(
                "Terminated contract restriction must be permanent and cannot expire".to_string(),
            );
        }

        self.b_load_executable_contract(contract_address)
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "Contract lifecycle restriction target {} is invalid: {}",
                    contract_address.to_base58(),
                    error
                )
            })
    }

    fn refresh_contract_lifecycle_for_restriction_record(
        &self,
        record: &RestrictionRecord,
        current_slot: u64,
    ) -> Result<(), String> {
        let RestrictionTarget::Contract(contract_address) = record.target else {
            return Ok(());
        };
        if contract_lifecycle_status_for_restriction_mode(&record.mode).is_some() {
            self.refresh_contract_lifecycle_from_restrictions(&contract_address, current_slot)?;
        }
        Ok(())
    }

    fn execute_restrict_governance_action(
        &self,
        proposal: &GovernanceProposal,
        action: RestrictGovernanceAction,
    ) -> Result<RestrictionRecord, String> {
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let RestrictGovernanceAction {
            target,
            mode,
            reason,
            evidence_hash,
            evidence_uri_hash,
            expires_at_slot,
        } = action;
        self.validate_contract_lifecycle_restriction_action(&target, &mode, expires_at_slot)?;
        let record = RestrictionRecord {
            id: self.b_next_restriction_id()?,
            target,
            mode,
            status: RestrictionStatus::Active,
            reason,
            evidence_hash,
            evidence_uri_hash,
            proposer: proposal.proposer,
            authority: proposal.authority,
            approval_authority: proposal.approval_authority,
            created_slot: current_slot,
            created_epoch: slot_to_epoch(current_slot),
            expires_at_slot,
            supersedes: None,
            lifted_by: None,
            lifted_slot: None,
            lift_reason: None,
        };

        self.b_put_restriction(&record)?;
        self.refresh_contract_lifecycle_for_restriction_record(&record, current_slot)?;
        Ok(record)
    }

    fn execute_lift_restriction_governance_action(
        &self,
        proposal: &GovernanceProposal,
        restriction_id: u64,
        reason: RestrictionLiftReason,
    ) -> Result<RestrictionRecord, String> {
        if restriction_id == 0 {
            return Err("LiftRestriction restriction_id must be greater than zero".to_string());
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let effective = self
            .b_get_effective_restriction_record(restriction_id, current_slot)?
            .ok_or_else(|| format!("Restriction {} not found", restriction_id))?;
        if !effective.is_active() {
            return Err(format!(
                "Restriction {} is not active at slot {}",
                restriction_id, current_slot
            ));
        }

        if matches!(&effective.record.mode, RestrictionMode::Terminated) {
            return Err(format!(
                "Terminated contract restriction {} cannot be lifted",
                restriction_id
            ));
        }

        let mut record = effective.record;
        record.status = RestrictionStatus::Lifted;
        record.lifted_by = Some(proposal.authority);
        record.lifted_slot = Some(current_slot);
        record.lift_reason = Some(reason);

        self.b_put_restriction(&record)?;
        self.refresh_contract_lifecycle_for_restriction_record(&record, current_slot)?;
        Ok(record)
    }

    fn execute_extend_restriction_governance_action(
        &self,
        proposal: &GovernanceProposal,
        restriction_id: u64,
        new_expires_at_slot: Option<u64>,
        evidence_hash: Option<Hash>,
    ) -> Result<RestrictionRecord, String> {
        if restriction_id == 0 {
            return Err("ExtendRestriction restriction_id must be greater than zero".to_string());
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let effective = self
            .b_get_effective_restriction_record(restriction_id, current_slot)?
            .ok_or_else(|| format!("Restriction {} not found", restriction_id))?;
        if !effective.is_active() {
            return Err(format!(
                "Restriction {} is not active at slot {}",
                restriction_id, current_slot
            ));
        }

        let old_record = effective.record;
        if matches!(&old_record.mode, RestrictionMode::Terminated) {
            return Err(format!(
                "Terminated contract restriction {} cannot be extended",
                restriction_id
            ));
        }
        let old_expires_at_slot = old_record
            .expires_at_slot
            .ok_or_else(|| format!("Restriction {} has no expiry to extend", restriction_id))?;
        let new_expires_at_slot = new_expires_at_slot.ok_or_else(|| {
            format!(
                "ExtendRestriction {} requires new_expires_at_slot",
                restriction_id
            )
        })?;
        if new_expires_at_slot <= old_expires_at_slot {
            return Err(format!(
                "Restriction {} new expiry {} must be greater than current expiry {}",
                restriction_id, new_expires_at_slot, old_expires_at_slot
            ));
        }
        if new_expires_at_slot <= current_slot {
            return Err(format!(
                "Restriction {} new expiry {} must be after execution slot {}",
                restriction_id, new_expires_at_slot, current_slot
            ));
        }
        self.validate_contract_lifecycle_restriction_action(
            &old_record.target,
            &old_record.mode,
            Some(new_expires_at_slot),
        )?;

        let mut superseded = old_record.clone();
        superseded.status = RestrictionStatus::Superseded;

        let successor = RestrictionRecord {
            id: self.b_next_restriction_id()?,
            target: old_record.target,
            mode: old_record.mode,
            status: RestrictionStatus::Active,
            reason: old_record.reason,
            evidence_hash: evidence_hash.or(old_record.evidence_hash),
            evidence_uri_hash: old_record.evidence_uri_hash,
            proposer: proposal.proposer,
            authority: proposal.authority,
            approval_authority: proposal.approval_authority,
            created_slot: current_slot,
            created_epoch: slot_to_epoch(current_slot),
            expires_at_slot: Some(new_expires_at_slot),
            supersedes: Some(old_record.id),
            lifted_by: None,
            lifted_slot: None,
            lift_reason: None,
        };

        superseded.validate()?;
        successor.validate()?;
        self.b_put_restriction(&superseded)?;
        self.b_put_restriction(&successor)?;
        self.refresh_contract_lifecycle_for_restriction_record(&successor, current_slot)?;
        Ok(successor)
    }

    pub(super) fn try_execute_governance_proposal(
        &self,
        proposal: &mut GovernanceProposal,
        actor: &Pubkey,
    ) -> Result<bool, String> {
        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        if !proposal.is_ready(current_epoch) {
            return Ok(false);
        }

        self.validate_governance_proposal_restriction_policy(proposal)?;

        let restriction_lifecycle_event = match proposal.action.clone() {
            GovernanceAction::TreasuryTransfer { recipient, amount } => {
                if !self.consume_governed_transfer_daily_capacity(
                    &proposal.authority,
                    amount,
                    proposal.daily_cap_spores,
                )? {
                    return Ok(false);
                }
                self.b_transfer(&proposal.authority, &recipient, amount)?;
                None
            }
            GovernanceAction::ParamChange { param_id, value } => {
                self.b_queue_governance_param_change(param_id, value)?;
                None
            }
            GovernanceAction::ContractUpgrade { contract, code } => {
                self.contract_upgrade_as_owner(&proposal.authority, &contract, code)?;
                None
            }
            GovernanceAction::SetContractUpgradeTimelock { contract, epochs } => {
                self.contract_set_upgrade_timelock_as_owner(
                    &proposal.authority,
                    &contract,
                    epochs,
                )?;
                None
            }
            GovernanceAction::ExecuteContractUpgrade { contract } => {
                self.contract_execute_upgrade_as_owner(&proposal.authority, &contract)?;
                None
            }
            GovernanceAction::VetoContractUpgrade { contract } => {
                self.contract_veto_upgrade_as_authority(&proposal.authority, &contract)?;
                None
            }
            GovernanceAction::ContractClose {
                contract,
                destination,
            } => {
                self.contract_close_as_owner(&proposal.authority, &contract, &destination)?;
                None
            }
            GovernanceAction::ContractCall {
                contract,
                function,
                args,
                value,
            } => {
                self.contract_call_as_caller(
                    &proposal.authority,
                    &contract,
                    &function,
                    &args,
                    value,
                )?;
                None
            }
            GovernanceAction::RegisterContractSymbol {
                contract,
                symbol,
                name,
                template,
                metadata,
                decimals,
            } => {
                self.register_symbol_as_owner(
                    &proposal.authority,
                    &contract,
                    SymbolRegistrationSpec {
                        symbol,
                        name,
                        template,
                        metadata,
                        decimals,
                    },
                )?;
                None
            }
            GovernanceAction::SetContractAbi { contract, abi } => {
                self.set_contract_abi_as_owner(&proposal.authority, &contract, abi)?;
                None
            }
            GovernanceAction::Restrict {
                target,
                mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            } => {
                let record = self.execute_restrict_governance_action(
                    proposal,
                    RestrictGovernanceAction {
                        target,
                        mode,
                        reason,
                        evidence_hash,
                        evidence_uri_hash,
                        expires_at_slot,
                    },
                )?;
                Some(("RestrictionCreated", record))
            }
            GovernanceAction::LiftRestriction {
                restriction_id,
                reason,
            } => {
                let record = self.execute_lift_restriction_governance_action(
                    proposal,
                    restriction_id,
                    reason,
                )?;
                Some(("RestrictionLifted", record))
            }
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                evidence_hash,
            } => {
                let record = self.execute_extend_restriction_governance_action(
                    proposal,
                    restriction_id,
                    new_expires_at_slot,
                    evidence_hash,
                )?;
                Some(("RestrictionExtended", record))
            }
        };

        proposal.executed = true;
        if let Some((event_name, record)) = restriction_lifecycle_event {
            self.emit_restriction_lifecycle_event(event_name, proposal, actor, &record)?;
        }
        Ok(true)
    }

    /// System instruction type 34: Propose a protocol governance action.
    pub(super) fn system_propose_governance_action(&self, ix: &Instruction) -> Result<(), String> {
        let (proposer, requested_authority, action) = self.parse_governance_action(ix)?;
        let (authority, approval_authority, approval_config) =
            self.resolve_governance_proposal_authority(&requested_authority, &action)?;

        if !approval_config.is_authorized(&proposer) {
            return Err(format!(
                "Proposer {} is not authorized for governance proposal authority {}",
                proposer.to_base58(),
                approval_config.label
            ));
        }

        if let GovernanceAction::TreasuryTransfer { amount, .. } = &action {
            let source_acct = self
                .b_get_account(&authority)?
                .ok_or_else(|| "Governance treasury account not found".to_string())?;
            if source_acct.spendable < *amount {
                return Err(format!(
                    "Governance treasury has insufficient spendable balance: {} < {}",
                    source_acct.spendable, amount
                ));
            }
        }

        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        let mut execution_policy = GovernedTransferExecutionPolicy {
            threshold: approval_config.threshold,
            execute_after_epoch: current_epoch + approval_config.timelock_epochs as u64,
            velocity_tier: crate::multisig::GovernedTransferVelocityTier::Standard,
            daily_cap_spores: 0,
        };
        if approval_authority.is_some()
            && self.governance_action_uses_immediate_risk_reduction_policy(&action)?
        {
            execution_policy.execute_after_epoch = current_epoch;
        } else if let GovernanceAction::TreasuryTransfer { amount, .. } = &action {
            let source_config = self
                .state
                .get_governed_wallet_config(&authority)
                .map_err(|e| format!("Failed to load governance treasury policy: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Governance treasury source {} is not configured as a governed wallet",
                        authority.to_base58()
                    )
                })?;
            execution_policy = self.governance_treasury_transfer_policy_snapshot(
                &approval_config,
                &source_config,
                *amount,
                current_epoch,
            )?;
        }
        let proposal_id = self
            .b_next_governance_proposal_id()
            .map_err(|e| format!("Failed to get governance proposal ID: {}", e))?;

        let mut proposal = GovernanceProposal {
            id: proposal_id,
            authority,
            approval_authority,
            proposer,
            action_label: action.label().to_string(),
            metadata: action.metadata(),
            action,
            approvals: vec![proposer],
            threshold: execution_policy.threshold,
            execute_after_epoch: execution_policy.execute_after_epoch,
            velocity_tier: execution_policy.velocity_tier,
            daily_cap_spores: execution_policy.daily_cap_spores,
            executed: false,
            cancelled: false,
        };

        self.try_execute_governance_proposal(&mut proposal, &proposer)?;
        self.b_set_governance_proposal(&proposal)
            .map_err(|e| format!("Failed to store governance proposal: {}", e))?;
        self.emit_governance_proposal_event("GovernanceProposalCreated", &proposal, &proposer)?;
        if proposal.executed {
            self.emit_governance_proposal_event(
                "GovernanceProposalExecuted",
                &proposal,
                &proposer,
            )?;
        }
        Ok(())
    }

    /// System instruction type 35: Approve a protocol governance action.
    pub(super) fn system_approve_governance_action(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ApproveGovernanceAction requires [approver]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("ApproveGovernanceAction: missing proposal_id".to_string());
        }

        let approver = ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governance_proposal(proposal_id)
            .map_err(|e| format!("Failed to load governance proposal: {}", e))?
            .ok_or_else(|| format!("Governance proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governance proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!("Governance proposal {} was cancelled", proposal_id));
        }

        let (approval_authority, config) =
            self.get_governance_proposal_approval_authority(&proposal)?;
        if !config.is_authorized(&approver) {
            return Err(format!(
                "Approver {} is not authorized for governance proposal approval authority {}",
                approver.to_base58(),
                approval_authority.to_base58()
            ));
        }
        if proposal.approvals.contains(&approver) {
            return Err(format!(
                "Approver {} has already approved governance proposal {}",
                approver.to_base58(),
                proposal_id
            ));
        }

        let was_executed = proposal.executed;
        proposal.approvals.push(approver);
        self.try_execute_governance_proposal(&mut proposal, &approver)?;
        self.b_set_governance_proposal(&proposal)
            .map_err(|e| format!("Failed to update governance proposal: {}", e))?;
        self.emit_governance_proposal_event("GovernanceProposalApproved", &proposal, &approver)?;
        if !was_executed && proposal.executed {
            self.emit_governance_proposal_event(
                "GovernanceProposalExecuted",
                &proposal,
                &approver,
            )?;
        }
        Ok(())
    }

    /// System instruction type 36: Execute a ready governance proposal.
    pub(super) fn system_execute_governance_action(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ExecuteGovernanceAction requires [executor]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("ExecuteGovernanceAction: missing proposal_id".to_string());
        }

        let executor = ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governance_proposal(proposal_id)
            .map_err(|e| format!("Failed to load governance proposal: {}", e))?
            .ok_or_else(|| format!("Governance proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governance proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!("Governance proposal {} was cancelled", proposal_id));
        }

        let (approval_authority, config) =
            self.get_governance_proposal_approval_authority(&proposal)?;
        if !config.is_authorized(&executor) {
            return Err(format!(
                "Executor {} is not authorized for governance proposal approval authority {}",
                executor.to_base58(),
                approval_authority.to_base58()
            ));
        }
        if proposal.approvals.len() < proposal.threshold as usize {
            return Err(format!(
                "Governance proposal {} has {} approvals but needs {}",
                proposal_id,
                proposal.approvals.len(),
                proposal.threshold
            ));
        }

        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        if current_epoch < proposal.execute_after_epoch {
            return Err(format!(
                "Governance proposal {} is timelocked until epoch {}",
                proposal_id, proposal.execute_after_epoch
            ));
        }

        let was_executed = proposal.executed;
        if !self.try_execute_governance_proposal(&mut proposal, &executor)? {
            let error = match proposal.action {
                GovernanceAction::TreasuryTransfer { amount, .. } => self
                    .governed_transfer_daily_cap_error(
                        &proposal.authority,
                        amount,
                        proposal.daily_cap_spores,
                    )?,
                _ => format!(
                    "Governance proposal {} is not ready for execution",
                    proposal_id
                ),
            };
            return Err(error);
        }
        self.b_set_governance_proposal(&proposal)
            .map_err(|e| format!("Failed to update governance proposal: {}", e))?;
        if !was_executed && proposal.executed {
            self.emit_governance_proposal_event(
                "GovernanceProposalExecuted",
                &proposal,
                &executor,
            )?;
        }
        Ok(())
    }

    /// System instruction type 37: Cancel a governance proposal before execution.
    pub(super) fn system_cancel_governance_action(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("CancelGovernanceAction requires [canceller]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("CancelGovernanceAction: missing proposal_id".to_string());
        }

        let canceller = ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governance_proposal(proposal_id)
            .map_err(|e| format!("Failed to load governance proposal: {}", e))?
            .ok_or_else(|| format!("Governance proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governance proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!(
                "Governance proposal {} already cancelled",
                proposal_id
            ));
        }

        let (approval_authority, config) =
            self.get_governance_proposal_approval_authority(&proposal)?;
        if !config.is_authorized(&canceller) {
            return Err(format!(
                "Canceller {} is not authorized for governance proposal approval authority {}",
                canceller.to_base58(),
                approval_authority.to_base58()
            ));
        }

        proposal.cancelled = true;
        self.b_set_governance_proposal(&proposal)
            .map_err(|e| format!("Failed to update governance proposal: {}", e))?;
        self.emit_governance_proposal_event("GovernanceProposalCancelled", &proposal, &canceller)?;
        Ok(())
    }
}
