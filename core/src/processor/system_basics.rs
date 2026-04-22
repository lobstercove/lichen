use super::*;

impl TxProcessor {
    /// Execute a single instruction.
    pub(super) fn execute_instruction(&self, ix: &Instruction) -> Result<(), String> {
        if ix.program_id == SYSTEM_PROGRAM_ID {
            self.execute_system_program(ix)
        } else if ix.program_id == CONTRACT_PROGRAM_ID {
            self.execute_contract_program(ix)
        } else {
            Err(format!("Unknown program: {}", ix.program_id))
        }
    }

    /// Execute system program instruction.
    pub(super) fn execute_system_program(&self, ix: &Instruction) -> Result<(), String> {
        if ix.data.is_empty() {
            return Err("Empty instruction data".to_string());
        }

        let instruction_type = ix.data[0];
        match instruction_type {
            0 => self.system_transfer(ix),
            2..=5 => {
                if let Some(sender) = ix.accounts.first() {
                    let is_treasury = self
                        .state
                        .get_treasury_pubkey()
                        .ok()
                        .flatten()
                        .map(|treasury| treasury == *sender)
                        .unwrap_or(false);
                    if !is_treasury {
                        return Err(format!(
                            "Instruction type {} restricted to treasury account",
                            instruction_type
                        ));
                    }
                }
                self.system_transfer(ix)
            }
            1 => self.system_create_account(ix),
            6 => self.system_create_collection(ix),
            7 => self.system_mint_nft(ix),
            8 => self.system_transfer_nft(ix),
            9 => self.system_stake(ix),
            10 => self.system_request_unstake(ix),
            11 => self.system_claim_unstake(ix),
            12 => self.system_register_evm_address(ix),
            13 => self.system_mossstake_deposit(ix),
            14 => self.system_mossstake_unstake(ix),
            15 => self.system_mossstake_claim(ix),
            16 => self.system_mossstake_transfer(ix),
            17 => self.system_deploy_contract(ix),
            18 => self.system_set_contract_abi(ix),
            19 => self.system_faucet_airdrop(ix),
            20 => self.system_register_symbol(ix),
            21 => self.system_propose_governed_transfer(ix),
            22 => self.system_approve_governed_transfer(ix),
            #[cfg(feature = "zk")]
            23 => self.system_shield_deposit(ix),
            #[cfg(feature = "zk")]
            24 => self.system_unshield_withdraw(ix),
            #[cfg(feature = "zk")]
            25 => self.system_shielded_transfer(ix),
            26 => self.system_register_validator(ix),
            27 => self.system_slash_validator(ix),
            28 => self.system_nonce(ix),
            29 => self.system_governance_param_change(ix),
            30 => self.system_oracle_attestation(ix),
            31 => self.system_deregister_validator(ix),
            32 => self.system_execute_governed_transfer(ix),
            33 => self.system_cancel_governed_transfer(ix),
            34 => self.system_propose_governance_action(ix),
            35 => self.system_approve_governance_action(ix),
            36 => self.system_execute_governance_action(ix),
            37 => self.system_cancel_governance_action(ix),
            _ => Err(format!("Unknown system instruction: {}", instruction_type)),
        }
    }

    /// System program: Transfer spores
    pub(super) fn system_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Transfer requires 2 accounts".to_string());
        }

        if ix.data.len() < 9 {
            return Err("Invalid transfer data".to_string());
        }

        let from = &ix.accounts[0];
        let to = &ix.accounts[1];

        if self
            .state
            .get_governed_wallet_config(from)
            .ok()
            .flatten()
            .is_some()
        {
            let is_treasury = self
                .state
                .get_treasury_pubkey()
                .ok()
                .flatten()
                .map(|treasury| treasury == *from)
                .unwrap_or(false);
            if !is_treasury {
                return Err(format!(
                    "Transfer from governed wallet {} requires multi-sig proposal (use types 21/22/32/33)",
                    from.to_base58()
                ));
            }
        }

        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        if amount == 0 {
            return Err("Transfer amount must be > 0".to_string());
        }

        self.b_transfer(from, to, amount)
    }

    /// System program: Create account
    pub(super) fn system_create_account(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("Create account requires at least 1 account".to_string());
        }

        let pubkey = &ix.accounts[0];
        if self.b_get_account(pubkey)?.is_some() {
            return Err("Account already exists".to_string());
        }

        let account = Account::new(0, *pubkey);
        self.b_put_account(pubkey, &account)?;

        Ok(())
    }

    /// System program: Register EVM address mapping
    pub(super) fn system_register_evm_address(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("Register EVM address requires signer account".to_string());
        }

        if ix.data.len() != 21 {
            return Err("Invalid EVM address data".to_string());
        }

        let mut evm_address = [0u8; 20];
        evm_address.copy_from_slice(&ix.data[1..21]);

        let native_pubkey = ix.accounts[0];
        if let Some(existing) = self.state.lookup_evm_address(&evm_address)? {
            if existing != native_pubkey {
                return Err("EVM address already mapped".to_string());
            }
            return Ok(());
        }

        self.b_register_evm_address(&evm_address, &native_pubkey)
    }

    /// System program: Register symbol for an existing deployed contract (instruction type 20).
    pub(super) fn system_register_symbol(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("RegisterSymbol requires [owner, contract_id] accounts".to_string());
        }
        if ix.data.len() < 2 {
            return Err("RegisterSymbol: missing symbol data".to_string());
        }

        let owner = ix.accounts[0];
        let contract_id = ix.accounts[1];

        let account = self
            .b_get_account(&contract_id)?
            .ok_or_else(|| "Contract account not found".to_string())?;
        if !account.executable {
            return Err("Account is not a deployed contract".to_string());
        }
        let contract: crate::ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to decode contract: {}", e))?;
        if contract.owner != owner {
            return Err("Only the contract owner can register a symbol".to_string());
        }

        if self.contract_owner_requires_governance_flow(&owner)? {
            return Err(
                "Governed contract owner must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        let registration = self.parse_symbol_registration_fields(&ix.data[1..])?;
        self.register_symbol_as_owner(&owner, &contract_id, registration)
    }

    pub(super) fn register_symbol_as_owner(
        &self,
        owner: &Pubkey,
        contract_id: &Pubkey,
        registration: SymbolRegistrationSpec,
    ) -> Result<(), String> {
        let account = self
            .b_get_account(contract_id)?
            .ok_or_else(|| "Contract account not found".to_string())?;
        if !account.executable {
            return Err("Account is not a deployed contract".to_string());
        }
        let contract: crate::ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to decode contract: {}", e))?;
        if contract.owner != *owner {
            return Err("Only the contract owner can register a symbol".to_string());
        }

        if let Ok(Some(_existing)) = self.state.get_symbol_registry_by_program(contract_id) {
            // Allow re-registration by the same owner/program.
        }

        {
            let batch_lock = self.batch.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref batch) = *batch_lock {
                if batch.symbol_exists(&registration.symbol).unwrap_or(false) {
                    if let Ok(Some(existing)) = batch.get_symbol_registry(&registration.symbol) {
                        if existing.program != *contract_id {
                            return Err(format!(
                                "Symbol '{}' is already registered by program {}",
                                registration.symbol,
                                existing.program.to_base58()
                            ));
                        }
                    } else {
                        return Err(format!(
                            "Symbol '{}' was already registered in this transaction batch",
                            registration.symbol
                        ));
                    }
                }
            } else if let Ok(Some(existing)) = self.state.get_symbol_registry(&registration.symbol)
            {
                if existing.program != *contract_id {
                    return Err(format!(
                        "Symbol '{}' is already registered by program {}",
                        registration.symbol,
                        existing.program.to_base58()
                    ));
                }
            }
        }

        let entry = SymbolRegistryEntry {
            symbol: registration.symbol.clone(),
            program: *contract_id,
            owner: *owner,
            name: registration.name,
            template: registration.template,
            metadata: registration.metadata,
            decimals: registration.decimals,
        };

        self.b_register_symbol(&registration.symbol, entry)
    }
}
