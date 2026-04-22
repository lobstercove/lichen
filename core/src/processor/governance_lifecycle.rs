use super::*;
use std::collections::HashMap;

impl TxProcessor {
    pub(super) fn emit_governance_proposal_event(
        &self,
        event_name: &str,
        proposal: &GovernanceProposal,
        actor: &Pubkey,
    ) -> Result<(), String> {
        let current_slot = self.b_get_last_slot().unwrap_or(0);
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

    pub(super) fn try_execute_governance_proposal(
        &self,
        proposal: &mut GovernanceProposal,
    ) -> Result<bool, String> {
        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        if !proposal.is_ready(current_epoch) {
            return Ok(false);
        }

        match proposal.action.clone() {
            GovernanceAction::TreasuryTransfer { recipient, amount } => {
                if !self.consume_governed_transfer_daily_capacity(
                    &proposal.authority,
                    amount,
                    proposal.daily_cap_spores,
                )? {
                    return Ok(false);
                }
                self.b_transfer(&proposal.authority, &recipient, amount)?;
            }
            GovernanceAction::ParamChange { param_id, value } => {
                self.b_queue_governance_param_change(param_id, value)?;
            }
            GovernanceAction::ContractUpgrade { contract, code } => {
                self.contract_upgrade_as_owner(&proposal.authority, &contract, code)?;
            }
            GovernanceAction::SetContractUpgradeTimelock { contract, epochs } => {
                self.contract_set_upgrade_timelock_as_owner(
                    &proposal.authority,
                    &contract,
                    epochs,
                )?;
            }
            GovernanceAction::ExecuteContractUpgrade { contract } => {
                self.contract_execute_upgrade_as_owner(&proposal.authority, &contract)?;
            }
            GovernanceAction::VetoContractUpgrade { contract } => {
                self.contract_veto_upgrade_as_authority(&proposal.authority, &contract)?;
            }
            GovernanceAction::ContractClose {
                contract,
                destination,
            } => {
                self.contract_close_as_owner(&proposal.authority, &contract, &destination)?;
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
            }
            GovernanceAction::SetContractAbi { contract, abi } => {
                self.set_contract_abi_as_owner(&proposal.authority, &contract, abi)?;
            }
        }

        proposal.executed = true;
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

        self.try_execute_governance_proposal(&mut proposal)?;
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
        self.try_execute_governance_proposal(&mut proposal)?;
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
        if !self.try_execute_governance_proposal(&mut proposal)? {
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
