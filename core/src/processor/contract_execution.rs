use super::contract_metadata::DeployRegistryData;
use super::*;

impl TxProcessor {
    /// Execute smart contract program instruction.
    pub(super) fn execute_contract_program(&self, ix: &Instruction) -> Result<(), String> {
        let contract_ix = ContractInstruction::deserialize(&ix.data)?;

        match contract_ix {
            ContractInstruction::Deploy { code, init_data } => {
                self.contract_deploy(ix, code, init_data)
            }
            ContractInstruction::Call {
                function,
                args,
                value,
            } => self.contract_call(ix, function, args, value),
            ContractInstruction::Upgrade { code } => self.contract_upgrade(ix, code),
            ContractInstruction::Close => self.contract_close(ix),
            ContractInstruction::SetUpgradeTimelock { epochs } => {
                self.contract_set_upgrade_timelock(ix, epochs)
            }
            ContractInstruction::ExecuteUpgrade => self.contract_execute_upgrade(ix),
            ContractInstruction::VetoUpgrade => self.contract_veto_upgrade(ix),
        }
    }

    /// Deploy smart contract.
    fn contract_deploy(
        &self,
        ix: &Instruction,
        code: Vec<u8>,
        init_data: Vec<u8>,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Deploy requires deployer and contract accounts".to_string());
        }

        if code.is_empty() {
            return Err("Deploy: code cannot be empty".to_string());
        }
        if code.len() > MAX_CONTRACT_CODE {
            return Err(format!(
                "Deploy: code size {} exceeds maximum {} bytes",
                code.len(),
                MAX_CONTRACT_CODE
            ));
        }
        if code.len() < 8 || code[..4] != [0x00, 0x61, 0x73, 0x6D] {
            return Err("Deploy: invalid WASM module (bad magic number)".to_string());
        }

        let deployer = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        tracing::debug!(
            "📋 contract_deploy: deployer={} addr={} code_len={}",
            deployer.to_base58(),
            contract_address.to_base58(),
            code.len()
        );

        if self.b_get_account(contract_address)?.is_some() {
            return Err(format!(
                "Contract account {} already exists (deployer={})",
                contract_address.to_base58(),
                deployer.to_base58()
            ));
        }

        let mut runtime = ContractRuntime::get_pooled();
        let deploy_result = runtime.deploy(&code);
        runtime.return_to_pool();
        if let Err(ref error) = deploy_result {
            tracing::debug!(
                "❌ contract_deploy: WASM validation failed for {} — {}",
                contract_address.to_base58(),
                error
            );
        }
        deploy_result?;

        let mut owner = *deployer;
        let mut make_public = true;
        let mut deployer_abi: Option<ContractAbi> = None;

        let registry_parsed = if !init_data.is_empty() {
            match DeployRegistryData::from_init_data(&init_data) {
                Some(registry) => Some(registry),
                None => {
                    tracing::debug!(
                        "⚠️  contract_deploy: init_data ({} bytes) could not be parsed as registry metadata — \
                         symbol/name/template will NOT be registered",
                        init_data.len()
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Some(registry) = registry_parsed {
            if let Some(raw_owner) = registry.upgrade_authority.clone() {
                if raw_owner == "none" {
                    owner = SYSTEM_PROGRAM_ID;
                } else if let Ok(custom_owner) = Pubkey::from_base58(&raw_owner) {
                    owner = custom_owner;
                }
            }

            if let Some(flag) = registry.make_public {
                make_public = flag;
            }

            deployer_abi = registry.abi.clone();

            if let Some(symbol) = registry.symbol.clone() {
                let entry = SymbolRegistryEntry {
                    symbol,
                    program: *contract_address,
                    owner,
                    name: registry.name.clone(),
                    template: registry.template.clone(),
                    metadata: registry.metadata.clone(),
                    decimals: registry.decimals,
                };
                self.b_register_symbol(&entry.symbol.clone(), entry)?;
            }
        }

        let mut contract = ContractAccount::new(code, owner);
        if let Some(abi) = deployer_abi {
            contract.abi = Some(abi);
        }

        let mut account = Account::new(0, *contract_address);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        account.executable = true;

        self.b_put_account(contract_address, &account)?;
        if make_public {
            self.b_index_program(contract_address)?;
        }

        tracing::debug!(
            "✅ contract_deploy: {} created (deployer={}, code={}B, data={}B)",
            contract_address.to_base58(),
            deployer.to_base58(),
            account.data.len(),
            init_data.len()
        );

        Ok(())
    }

    /// Call smart contract function.
    fn contract_call(
        &self,
        ix: &Instruction,
        function: String,
        args: Vec<u8>,
        value: u64,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Call requires caller and contract accounts".to_string());
        }

        let caller = ix.accounts[0];
        let contract_address = ix.accounts[1];

        if matches!(
            self.get_governed_governance_authority()?,
            Some((authority, _)) if authority == caller
        ) {
            return Err(
                "Governed governance authority must use governance action proposal flow (use types 34-37)"
                    .to_string(),
            );
        }

        self.contract_call_as_caller(&caller, &contract_address, &function, &args, value)
    }

    pub(super) fn contract_call_as_caller(
        &self,
        caller: &Pubkey,
        contract_address: &Pubkey,
        function: &str,
        args: &[u8],
        value: u64,
    ) -> Result<(), String> {
        let args = args.to_vec();

        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        if !account.executable {
            return Err("Account is not a contract".to_string());
        }

        let contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if value > 0 {
            self.b_transfer(caller, contract_address, value)?;
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let tx_budget = *self
            .tx_compute_budget
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let cu_used_so_far = self
            .contract_meta
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .2;
        let context = build_top_level_call_context(
            ContractContext::with_args(
                *caller,
                *contract_address,
                value,
                current_slot,
                self.b_load_contract_storage_map(contract_address)?,
                args,
            ),
            self.state.clone(),
            tx_budget.saturating_sub(cu_used_so_far),
        );

        let mut runtime = ContractRuntime::get_pooled();
        let result = runtime.execute(&contract, function, &context.args.clone(), context)?;
        runtime.return_to_pool();

        {
            let mut meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
            meta.0 = result.return_code;
            meta.1.extend(result.logs.iter().cloned());
            meta.1.extend(result.cross_call_logs.iter().cloned());
            meta.2 = meta.2.saturating_add(result.compute_used);
            meta.3 = result.return_data.clone();
        }

        if result.success {
            if let Some(rc) = result.return_code {
                let meaningful_changes = result
                    .storage_changes
                    .keys()
                    .any(|key| !key.ends_with(b"_reentrancy"));
                if rc != 0 && !meaningful_changes && result.cross_call_changes.is_empty() {
                    return Err(format!(
                        "Contract '{}' returned error code {} with no state changes. Logs: {:?}",
                        function, rc, result.logs
                    ));
                }
            }
        }

        if !result.success {
            return Err(result
                .error
                .unwrap_or("Contract execution failed".to_string()));
        }

        for (addr, delta) in &result.ccc_value_deltas {
            if *delta == 0 {
                continue;
            }
            let mut account = self
                .b_get_account(addr)?
                .ok_or_else(|| format!("CCC value delta target {} not found", addr))?;
            if *delta > 0 {
                account.add_spendable(*delta as u64)?;
            } else {
                let abs = (-*delta) as u64;
                account.deduct_spendable(abs)?;
            }
            self.b_put_account(addr, &account)?;
        }

        for op in &result.native_account_ops {
            let address = op.account();
            let mut account = self
                .b_get_account(&address)?
                .ok_or_else(|| format!("Native account op target {} not found", address))?;
            op.apply(&mut account)?;
            self.b_put_account(&address, &account)?;

            if let Some(to_key) = op.transfer_to() {
                if let NativeAccountOp::Transfer { amount, .. } = op {
                    let mut to_account = self
                        .b_get_account(&to_key)?
                        .unwrap_or_else(|| Account::new(0, to_key));
                    to_account.spores = to_account.spores.saturating_add(*amount);
                    to_account.spendable = to_account.spendable.saturating_add(*amount);
                    self.b_put_account(&to_key, &to_account)?;
                }
            }
        }

        for event in &result.events {
            self.b_put_contract_event(contract_address, event)?;
        }

        for event in &result.cross_call_events {
            self.b_put_contract_event(&event.program, event)?;
        }

        if !result.storage_changes.is_empty() {
            for (key, value_opt) in &result.storage_changes {
                match value_opt {
                    Some(val) => {
                        self.b_put_contract_storage(contract_address, key, val)?;
                    }
                    None => {
                        self.b_delete_contract_storage(contract_address, key)?;
                    }
                }
            }
        }

        self.index_token_balances_from_map(contract_address, &result.storage_changes)?;

        for (target_addr, changes) in &result.cross_call_changes {
            if changes.is_empty() {
                continue;
            }

            let target_account = self
                .b_get_account(target_addr)?
                .ok_or_else(|| format!("Cross-call target {} not found", target_addr))?;
            let _: ContractAccount = serde_json::from_slice(&target_account.data)
                .map_err(|e| format!("Failed to deserialize cross-call target: {}", e))?;

            for (key, value_opt) in changes {
                match value_opt {
                    Some(val) => {
                        self.b_put_contract_storage(target_addr, key, val)?;
                    }
                    None => {
                        self.b_delete_contract_storage(target_addr, key)?;
                    }
                }
            }

            self.index_token_balances_from_map(target_addr, changes)?;
        }

        Ok(())
    }

    /// Scan storage changes for token balance keys (`_bal_` pattern) and update
    /// the token balance indexes (CF_TOKEN_BALANCES / CF_HOLDER_TOKENS).
    /// Key format in contracts: `{prefix}_bal_{64-hex-of-32-byte-address}` → u64 LE
    fn index_token_balances_from_map(
        &self,
        program: &Pubkey,
        changes: &HashMap<Vec<u8>, Option<Vec<u8>>>,
    ) -> Result<(), String> {
        for (key, value_opt) in changes {
            self.maybe_index_token_balance(program, key, value_opt)?;
        }
        Ok(())
    }

    /// Check a single storage key for `_bal_` pattern and update token balance index.
    fn maybe_index_token_balance(
        &self,
        program: &Pubkey,
        key: &[u8],
        value_opt: &Option<Vec<u8>>,
    ) -> Result<(), String> {
        let key_str = match std::str::from_utf8(key) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        if let Some(pos) = key_str.find("_bal_") {
            let hex_part = &key_str[pos + 5..];
            if hex_part.len() != 64 {
                return Ok(());
            }
            let mut holder_bytes = [0u8; 32];
            if hex::decode_to_slice(hex_part, &mut holder_bytes).is_err() {
                return Ok(());
            }
            let holder = Pubkey(holder_bytes);
            let balance = match value_opt {
                Some(val) if val.len() == 8 => {
                    u64::from_le_bytes(val.as_slice().try_into().unwrap())
                }
                None => 0,
                _ => return Ok(()),
            };
            self.b_update_token_balance(program, &holder, balance)?;
        }
        Ok(())
    }
}
