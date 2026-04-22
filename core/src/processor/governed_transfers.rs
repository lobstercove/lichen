use super::*;

impl TxProcessor {
    /// System instruction type 21: Propose a governed transfer.
    pub(super) fn system_propose_governed_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 3 {
            return Err(
                "ProposeGovernedTransfer requires [proposer, source, recipient]".to_string(),
            );
        }
        if ix.data.len() < 9 {
            return Err("ProposeGovernedTransfer: missing amount".to_string());
        }

        let proposer = &ix.accounts[0];
        let source = &ix.accounts[1];
        let recipient = &ix.accounts[2];

        let amount = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid amount encoding".to_string())?,
        );

        let config = self
            .state
            .get_governed_wallet_config(source)
            .map_err(|e| format!("Failed to load governed wallet config: {}", e))?
            .ok_or_else(|| format!("Account {} is not a governed wallet", source.to_base58()))?;

        if !config.is_authorized(proposer) {
            return Err(format!(
                "Proposer {} is not an authorized signer for governed wallet {}",
                proposer.to_base58(),
                config.label
            ));
        }

        let source_acct = self
            .b_get_account(source)?
            .ok_or_else(|| "Governed wallet account not found".to_string())?;
        if source_acct.spendable < amount {
            return Err(format!(
                "Governed wallet has insufficient spendable balance: {} < {}",
                source_acct.spendable, amount
            ));
        }

        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        let execution_policy =
            self.governed_transfer_policy_snapshot(&config, amount, current_epoch)?;

        let proposal_id = self
            .b_next_governed_proposal_id()
            .map_err(|e| format!("Failed to get proposal ID: {}", e))?;

        let mut proposal = crate::multisig::GovernedProposal {
            id: proposal_id,
            source: *source,
            recipient: *recipient,
            amount,
            approvals: vec![*proposer],
            threshold: execution_policy.threshold,
            execute_after_epoch: execution_policy.execute_after_epoch,
            velocity_tier: execution_policy.velocity_tier,
            daily_cap_spores: execution_policy.daily_cap_spores,
            executed: false,
            cancelled: false,
        };

        self.try_execute_governed_proposal(&mut proposal)?;

        self.b_set_governed_proposal(&proposal)
            .map_err(|e| format!("Failed to store proposal: {}", e))?;

        Ok(())
    }

    /// System instruction type 22: Approve a governed transfer proposal.
    pub(super) fn system_approve_governed_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ApproveGovernedTransfer requires [approver]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("ApproveGovernedTransfer: missing proposal_id".to_string());
        }

        let approver = &ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governed_proposal(proposal_id)
            .map_err(|e| format!("Failed to load proposal: {}", e))?
            .ok_or_else(|| format!("Governed proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governed proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!("Governed proposal {} was cancelled", proposal_id));
        }

        let config = self
            .state
            .get_governed_wallet_config(&proposal.source)
            .map_err(|e| format!("Failed to load governed wallet config: {}", e))?
            .ok_or_else(|| "Source is no longer a governed wallet".to_string())?;

        if !config.is_authorized(approver) {
            return Err(format!(
                "Approver {} is not authorized for this governed wallet",
                approver.to_base58()
            ));
        }

        if proposal.approvals.contains(approver) {
            return Err(format!(
                "Approver {} has already approved proposal {}",
                approver.to_base58(),
                proposal_id
            ));
        }

        proposal.approvals.push(*approver);

        self.try_execute_governed_proposal(&mut proposal)?;

        self.b_set_governed_proposal(&proposal)
            .map_err(|e| format!("Failed to update proposal: {}", e))?;

        Ok(())
    }

    /// System instruction type 32: Execute a governed transfer proposal once satisfied.
    pub(super) fn system_execute_governed_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ExecuteGovernedTransfer requires [executor]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("ExecuteGovernedTransfer: missing proposal_id".to_string());
        }

        let executor = &ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governed_proposal(proposal_id)
            .map_err(|e| format!("Failed to load proposal: {}", e))?
            .ok_or_else(|| format!("Governed proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governed proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!("Governed proposal {} was cancelled", proposal_id));
        }

        let config = self
            .state
            .get_governed_wallet_config(&proposal.source)
            .map_err(|e| format!("Failed to load governed wallet config: {}", e))?
            .ok_or_else(|| "Source is no longer a governed wallet".to_string())?;

        if !config.is_authorized(executor) {
            return Err(format!(
                "Executor {} is not authorized for this governed wallet",
                executor.to_base58()
            ));
        }

        if proposal.approvals.len() < proposal.threshold as usize {
            return Err(format!(
                "Governed proposal {} has {} approvals but needs {}",
                proposal_id,
                proposal.approvals.len(),
                proposal.threshold
            ));
        }

        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        if current_epoch < proposal.execute_after_epoch {
            return Err(format!(
                "Governed proposal {} is timelocked until epoch {}",
                proposal_id, proposal.execute_after_epoch
            ));
        }

        if !self.try_execute_governed_proposal(&mut proposal)? {
            return Err(self.governed_transfer_daily_cap_error(
                &proposal.source,
                proposal.amount,
                proposal.daily_cap_spores,
            )?);
        }
        self.b_set_governed_proposal(&proposal)
            .map_err(|e| format!("Failed to update proposal: {}", e))?;
        Ok(())
    }

    /// System instruction type 33: Cancel a governed transfer proposal before execution.
    pub(super) fn system_cancel_governed_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("CancelGovernedTransfer requires [canceller]".to_string());
        }
        if ix.data.len() < 9 {
            return Err("CancelGovernedTransfer: missing proposal_id".to_string());
        }

        let canceller = &ix.accounts[0];
        let proposal_id = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid proposal ID encoding".to_string())?,
        );

        let mut proposal = self
            .b_get_governed_proposal(proposal_id)
            .map_err(|e| format!("Failed to load proposal: {}", e))?
            .ok_or_else(|| format!("Governed proposal {} not found", proposal_id))?;

        if proposal.executed {
            return Err(format!(
                "Governed proposal {} already executed",
                proposal_id
            ));
        }
        if proposal.cancelled {
            return Err(format!(
                "Governed proposal {} already cancelled",
                proposal_id
            ));
        }

        let config = self
            .state
            .get_governed_wallet_config(&proposal.source)
            .map_err(|e| format!("Failed to load governed wallet config: {}", e))?
            .ok_or_else(|| "Source is no longer a governed wallet".to_string())?;

        if !config.is_authorized(canceller) {
            return Err(format!(
                "Canceller {} is not authorized for this governed wallet",
                canceller.to_base58()
            ));
        }

        proposal.cancelled = true;
        self.b_set_governed_proposal(&proposal)
            .map_err(|e| format!("Failed to update proposal: {}", e))?;
        Ok(())
    }

    fn try_execute_governed_proposal(
        &self,
        proposal: &mut crate::multisig::GovernedProposal,
    ) -> Result<bool, String> {
        let current_epoch = slot_to_epoch(self.b_get_last_slot().unwrap_or(0));
        if proposal.cancelled
            || proposal.executed
            || proposal.approvals.len() < proposal.threshold as usize
            || current_epoch < proposal.execute_after_epoch
        {
            return Ok(false);
        }

        if !self.consume_governed_transfer_daily_capacity(
            &proposal.source,
            proposal.amount,
            proposal.daily_cap_spores,
        )? {
            return Ok(false);
        }

        self.b_transfer(&proposal.source, &proposal.recipient, proposal.amount)?;
        proposal.executed = true;
        Ok(true)
    }

    fn governed_transfer_policy_snapshot_from_parts(
        threshold: u8,
        timelock_epochs: u32,
        signer_count: usize,
        label: &str,
        transfer_velocity_policy: Option<&crate::multisig::GovernedTransferVelocityPolicy>,
        amount: u64,
        current_epoch: u64,
    ) -> Result<GovernedTransferExecutionPolicy, String> {
        let mut execution_policy = GovernedTransferExecutionPolicy {
            threshold,
            execute_after_epoch: current_epoch + timelock_epochs as u64,
            velocity_tier: crate::multisig::GovernedTransferVelocityTier::Standard,
            daily_cap_spores: 0,
        };

        if let Some(policy) = transfer_velocity_policy {
            if policy.per_transfer_cap_spores > 0 && amount > policy.per_transfer_cap_spores {
                return Err(format!(
                    "Governed transfer amount {} exceeds per-transfer cap {} for {}",
                    amount, policy.per_transfer_cap_spores, label
                ));
            }

            let tier = policy.tier_for_amount(amount);
            execution_policy.threshold = policy.required_threshold(threshold, signer_count, tier);
            execution_policy.execute_after_epoch +=
                u64::from(policy.additional_timelock_epochs(tier));
            execution_policy.velocity_tier = tier;
            execution_policy.daily_cap_spores = policy.daily_cap_spores;
        }

        Ok(execution_policy)
    }

    fn governed_transfer_policy_snapshot(
        &self,
        config: &crate::multisig::GovernedWalletConfig,
        amount: u64,
        current_epoch: u64,
    ) -> Result<GovernedTransferExecutionPolicy, String> {
        Self::governed_transfer_policy_snapshot_from_parts(
            config.threshold,
            config.timelock_epochs,
            config.signers.len(),
            config.label.as_str(),
            config.transfer_velocity_policy.as_ref(),
            amount,
            current_epoch,
        )
    }

    pub(super) fn governance_treasury_transfer_policy_snapshot(
        &self,
        approval_config: &crate::multisig::GovernedWalletConfig,
        source_config: &crate::multisig::GovernedWalletConfig,
        amount: u64,
        current_epoch: u64,
    ) -> Result<GovernedTransferExecutionPolicy, String> {
        Self::governed_transfer_policy_snapshot_from_parts(
            approval_config.threshold,
            approval_config.timelock_epochs,
            approval_config.signers.len(),
            source_config.label.as_str(),
            source_config.transfer_velocity_policy.as_ref(),
            amount,
            current_epoch,
        )
    }

    fn current_governed_transfer_day_bucket(&self) -> Result<u64, String> {
        let last_slot = self.b_get_last_slot().unwrap_or(0);
        let timestamp = self
            .state
            .get_block_by_slot(last_slot)?
            .map(|block| block.header.timestamp)
            .unwrap_or(last_slot);
        Ok(timestamp / SECONDS_PER_DAY)
    }

    pub(super) fn consume_governed_transfer_daily_capacity(
        &self,
        source: &Pubkey,
        amount: u64,
        daily_cap_spores: u64,
    ) -> Result<bool, String> {
        if daily_cap_spores == 0 {
            return Ok(true);
        }

        let day_bucket = self.current_governed_transfer_day_bucket()?;
        let current_volume = self.b_get_governed_transfer_day_volume(source, day_bucket)?;
        let updated_volume = current_volume.checked_add(amount).ok_or_else(|| {
            format!(
                "Governed transfer volume overflow for {}",
                source.to_base58()
            )
        })?;
        if updated_volume > daily_cap_spores {
            return Ok(false);
        }

        self.b_set_governed_transfer_day_volume(source, day_bucket, updated_volume)?;
        Ok(true)
    }

    pub(super) fn governed_transfer_daily_cap_error(
        &self,
        source: &Pubkey,
        amount: u64,
        daily_cap_spores: u64,
    ) -> Result<String, String> {
        let day_bucket = self.current_governed_transfer_day_bucket()?;
        let current_volume = self.b_get_governed_transfer_day_volume(source, day_bucket)?;
        Ok(format!(
            "Governed transfer would exceed daily cap for {}: {} + {} > {} on day {}",
            source.to_base58(),
            current_volume,
            amount,
            daily_cap_spores,
            day_bucket
        ))
    }
}
