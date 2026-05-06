use super::*;

impl TxProcessor {
    pub(super) fn contract_owner_requires_governance_flow(
        &self,
        owner: &Pubkey,
    ) -> Result<bool, String> {
        Ok(matches!(
            self.get_governed_governance_authority()?,
            Some((authority, _)) if authority == *owner
        ))
    }

    pub(super) fn b_load_executable_contract(
        &self,
        contract_address: &Pubkey,
    ) -> Result<(Account, ContractAccount), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or_else(|| "Contract not found".to_string())?;

        if !account.executable {
            return Err("Account is not a contract".to_string());
        }

        let contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        Ok((account, contract))
    }

    pub(super) fn refresh_contract_lifecycle_from_restrictions(
        &self,
        contract_address: &Pubkey,
        current_slot: u64,
    ) -> Result<ContractAccount, String> {
        let (mut account, mut contract) = self.b_load_executable_contract(contract_address)?;
        let target = crate::restrictions::RestrictionTarget::Contract(*contract_address);
        let active_records = self.b_get_active_restrictions_for_target(&target, current_slot, 0)?;
        let linked_restriction_active = match contract.lifecycle_restriction_id {
            Some(id) => self
                .b_get_effective_restriction_record(id, current_slot)?
                .map(|effective| effective.is_active()),
            None => None,
        };

        if contract.sync_lifecycle_from_restrictions(
            &active_records,
            linked_restriction_active,
            current_slot,
        ) {
            account.data = serde_json::to_vec(&contract)
                .map_err(|e| format!("Failed to serialize contract: {}", e))?;
            self.b_put_account(contract_address, &account)?;
        }

        Ok(contract)
    }

    pub(super) fn ensure_code_hash_not_deploy_blocked(
        &self,
        code_hash: &Hash,
        current_slot: u64,
        operation: &str,
    ) -> Result<(), String> {
        let target = crate::restrictions::RestrictionTarget::CodeHash(*code_hash);
        let active_records = self.b_get_active_restrictions_for_target(&target, current_slot, 0)?;

        if let Some(record) = active_records.into_iter().find(|record| {
            matches!(
                record.mode,
                crate::restrictions::RestrictionMode::DeployBlocked
            )
        }) {
            return Err(format!(
                "{} rejected: code hash {} is blocked by active DeployBlocked restriction {}",
                operation,
                code_hash.to_hex(),
                record.id
            ));
        }

        Ok(())
    }

    /// Upgrade contract (owner only).
    /// If the contract has a timelock, the upgrade is staged rather than applied
    /// immediately. Without a timelock, behaviour is unchanged (instant upgrade).
    pub(super) fn contract_upgrade(
        &self,
        ix: &Instruction,
        new_code: Vec<u8>,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Upgrade requires owner and contract accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        if self.contract_owner_requires_governance_flow(owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.contract_upgrade_as_owner(owner, contract_address, new_code)
    }

    pub(super) fn contract_upgrade_as_owner(
        &self,
        owner: &Pubkey,
        contract_address: &Pubkey,
        new_code: Vec<u8>,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can upgrade".to_string());
        }

        let mut runtime = ContractRuntime::new();
        let new_hash = runtime.deploy(&new_code)?;
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        self.ensure_code_hash_not_deploy_blocked(&new_hash, current_slot, "ContractUpgrade")?;

        if let Some(timelock_epochs) = contract.upgrade_timelock_epochs {
            if timelock_epochs > 0 {
                if contract.pending_upgrade.is_some() {
                    return Err(
                        "Contract already has a pending upgrade — execute or veto first"
                            .to_string(),
                    );
                }
                let current_epoch = crate::consensus::slot_to_epoch(current_slot);
                contract.pending_upgrade = Some(crate::contract::PendingUpgrade {
                    code: new_code,
                    code_hash: new_hash,
                    submitted_epoch: current_epoch,
                    execute_after_epoch: current_epoch + timelock_epochs as u64,
                });

                let mut updated_account = account;
                updated_account.data = serde_json::to_vec(&contract)
                    .map_err(|e| format!("Failed to serialize contract: {}", e))?;
                self.b_put_account(contract_address, &updated_account)?;

                return Ok(());
            }
        }

        contract.previous_code_hash = Some(contract.code_hash);
        contract.version = contract.version.saturating_add(1);
        contract.code = new_code;
        contract.code_hash = new_hash;
        contract.abi = None;
        contract.pending_upgrade = None;

        let mut updated_account = account;
        updated_account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;

        self.b_put_account(contract_address, &updated_account)?;

        Ok(())
    }

    /// Set or remove the upgrade timelock for a contract (owner only).
    pub(super) fn contract_set_upgrade_timelock(
        &self,
        ix: &Instruction,
        epochs: u32,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("SetUpgradeTimelock requires owner and contract accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        if self.contract_owner_requires_governance_flow(owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.contract_set_upgrade_timelock_as_owner(owner, contract_address, epochs)
    }

    pub(super) fn contract_set_upgrade_timelock_as_owner(
        &self,
        owner: &Pubkey,
        contract_address: &Pubkey,
        epochs: u32,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can set upgrade timelock".to_string());
        }

        if epochs == 0 && contract.pending_upgrade.is_some() {
            return Err(
                "Cannot remove timelock while an upgrade is pending — execute or veto first"
                    .to_string(),
            );
        }

        contract.upgrade_timelock_epochs = if epochs == 0 { None } else { Some(epochs) };

        let mut updated_account = account;
        updated_account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;

        self.b_put_account(contract_address, &updated_account)?;

        Ok(())
    }

    /// Execute a previously staged upgrade after the timelock has expired (owner only).
    pub(super) fn contract_execute_upgrade(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("ExecuteUpgrade requires owner and contract accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        if self.contract_owner_requires_governance_flow(owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.contract_execute_upgrade_as_owner(owner, contract_address)
    }

    pub(super) fn contract_execute_upgrade_as_owner(
        &self,
        owner: &Pubkey,
        contract_address: &Pubkey,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can execute upgrade".to_string());
        }

        let pending = contract
            .pending_upgrade
            .take()
            .ok_or("No pending upgrade to execute")?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let current_epoch = crate::consensus::slot_to_epoch(current_slot);

        if current_epoch <= pending.execute_after_epoch {
            return Err(format!(
                "Timelock has not expired — current epoch {} but upgrade executable after epoch {}",
                current_epoch, pending.execute_after_epoch,
            ));
        }

        self.ensure_code_hash_not_deploy_blocked(
            &pending.code_hash,
            current_slot,
            "ExecuteContractUpgrade",
        )?;

        contract.previous_code_hash = Some(contract.code_hash);
        contract.version = contract.version.saturating_add(1);
        contract.code = pending.code;
        contract.code_hash = pending.code_hash;
        contract.abi = None;

        let mut updated_account = account;
        updated_account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;

        self.b_put_account(contract_address, &updated_account)?;

        Ok(())
    }

    /// Veto (cancel) a pending contract upgrade. Governance authority only.
    pub(super) fn contract_veto_upgrade(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err(
                "VetoUpgrade requires governance authority and contract accounts".to_string(),
            );
        }

        let signer = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        let governance_authority = self
            .state
            .get_governance_authority()?
            .ok_or("No governance authority configured")?;

        if self
            .state
            .get_governed_wallet_config(&governance_authority)?
            .is_some()
        {
            return Err(
                "Governed governance authority must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        if *signer != governance_authority {
            return Err("Only governance authority can veto upgrades".to_string());
        }

        self.contract_veto_upgrade_as_authority(signer, contract_address)
    }

    pub(super) fn contract_veto_upgrade_as_authority(
        &self,
        authority: &Pubkey,
        contract_address: &Pubkey,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.pending_upgrade.is_none() {
            return Err("No pending upgrade to veto".to_string());
        }

        let governance_authority = self
            .state
            .get_governance_authority()?
            .ok_or("No governance authority configured")?;
        if *authority != governance_authority {
            return Err("Only governance authority can veto upgrades".to_string());
        }

        contract.pending_upgrade = None;

        let mut updated_account = account;
        updated_account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;

        self.b_put_account(contract_address, &updated_account)?;

        Ok(())
    }

    /// Close contract and withdraw balance.
    pub(super) fn contract_close(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 3 {
            return Err("Close requires owner, contract, and destination accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];
        let destination = &ix.accounts[2];

        if self.contract_owner_requires_governance_flow(owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.contract_close_as_owner(owner, contract_address, destination)
    }

    pub(super) fn contract_close_as_owner(
        &self,
        owner: &Pubkey,
        contract_address: &Pubkey,
        destination: &Pubkey,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can close".to_string());
        }

        if account.staked > 0 {
            return Err(format!(
                "Cannot close contract with {} staked spores — unstake first",
                account.staked
            ));
        }
        if account.locked > 0 {
            return Err(format!(
                "Cannot close contract with {} locked spores — claim unstake first",
                account.locked
            ));
        }

        let spendable = account.spendable;
        if spendable > 0 {
            self.b_transfer(contract_address, destination, spendable)?;
        }

        let mut closed_account = self.b_get_account(contract_address)?.unwrap_or(account);
        closed_account.executable = false;
        closed_account.data = Vec::new();
        self.b_put_account(contract_address, &closed_account)
    }
}
