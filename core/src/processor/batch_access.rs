use super::*;

impl TxProcessor {
    pub(super) fn b_get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_account(pubkey)
        } else {
            self.state.get_account(pubkey)
        }
    }

    pub(super) fn b_put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_account(pubkey, account)
        } else {
            self.state.put_account(pubkey, account)
        }
    }

    pub(super) fn b_transfer(&self, from: &Pubkey, to: &Pubkey, amount: u64) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.transfer(from, to, amount)
        } else {
            self.state.transfer(from, to, amount)
        }
    }

    pub(super) fn b_put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_transaction(tx)
        } else {
            self.state.put_transaction(tx)
        }
    }

    pub(super) fn b_has_transaction(&self, sig: &Hash) -> Result<bool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.has_transaction(sig)
        } else {
            Ok(self.state.get_transaction(sig)?.is_some())
        }
    }

    /// Store full transaction metadata, peeking at contract_meta for return_code/data/logs.
    /// Used for both success and failure paths (outside of batches).
    pub(super) fn store_tx_meta(&self, sig: &Hash, compute_units_used: u64) -> Result<(), String> {
        let tx_meta = {
            let meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
            TxMeta {
                compute_units_used,
                return_code: meta.0,
                return_data: meta.3.clone(),
                logs: meta.1.clone(),
            }
        };
        self.state.put_tx_meta_full(sig, &tx_meta)
    }

    pub(super) fn b_put_tx_meta(&self, sig: &Hash, compute_units_used: u64) -> Result<(), String> {
        let tx_meta = {
            let meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
            TxMeta {
                compute_units_used,
                return_code: meta.0,
                return_data: meta.3.clone(),
                logs: meta.1.clone(),
            }
        };
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_tx_meta_full(sig, &tx_meta)
        } else {
            self.state.put_tx_meta_full(sig, &tx_meta)
        }
    }

    pub(super) fn b_put_stake_pool(
        &self,
        pool: &crate::consensus::StakePool,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_stake_pool(pool)
        } else {
            self.state.put_stake_pool(pool)
        }
    }

    pub(super) fn b_get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_stake_pool()
        } else {
            self.state.get_stake_pool()
        }
    }

    pub(super) fn b_put_mossstake_pool(
        &self,
        pool: &crate::mossstake::MossStakePool,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_mossstake_pool(pool)
        } else {
            self.state.put_mossstake_pool(pool)
        }
    }

    pub(super) fn b_get_mossstake_pool(&self) -> Result<crate::mossstake::MossStakePool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_mossstake_pool()
        } else {
            self.state.get_mossstake_pool()
        }
    }

    pub(super) fn b_put_contract_event(
        &self,
        program: &Pubkey,
        event: &crate::contract::ContractEvent,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_contract_event(program, event)
        } else {
            self.state.put_contract_event(program, event)
        }
    }

    /// Write contract storage change to CF_CONTRACT_STORAGE for fast-path access.
    pub(super) fn b_put_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_contract_storage(program, storage_key, value)
        } else {
            self.state.put_contract_storage(program, storage_key, value)
        }
    }

    pub(super) fn b_get_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<Option<Vec<u8>>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_contract_storage(program, storage_key)
        } else {
            self.state.get_contract_storage(program, storage_key)
        }
    }

    pub(super) fn b_load_contract_storage_map(
        &self,
        program: &Pubkey,
    ) -> Result<HashMap<Vec<u8>, Vec<u8>>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            Ok(batch
                .load_contract_storage_map(program)?
                .into_iter()
                .collect())
        } else {
            Ok(self
                .state
                .load_contract_storage_map(program)?
                .into_iter()
                .collect())
        }
    }

    /// Delete contract storage key from CF_CONTRACT_STORAGE.
    pub(super) fn b_delete_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.delete_contract_storage(program, storage_key)
        } else {
            self.state.delete_contract_storage(program, storage_key)
        }
    }

    /// Update token balance indexes (forward + reverse) within the batch.
    pub(super) fn b_update_token_balance(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
        balance: u64,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.update_token_balance(token_program, holder, balance)
        } else {
            self.state
                .update_token_balance(token_program, holder, balance)
        }
    }

    pub(super) fn b_put_evm_tx(&self, record: &crate::evm::EvmTxRecord) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_evm_tx(record)
        } else {
            self.state.put_evm_tx(record)
        }
    }

    pub(super) fn b_put_evm_receipt(&self, receipt: &crate::evm::EvmReceipt) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_evm_receipt(receipt)
        } else {
            self.state.put_evm_receipt(receipt)
        }
    }

    /// Task 3.4: Store EVM logs in per-slot index through the active batch.
    pub(super) fn b_put_evm_logs_for_slot(
        &self,
        slot: u64,
        logs: &[crate::evm::EvmLogEntry],
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_evm_logs_for_slot(slot, logs)
        } else {
            self.state.put_evm_logs_for_slot(slot, logs)
        }
    }

    /// H3 fix: Apply deferred EVM state changes through the active batch.
    pub(super) fn b_apply_evm_state_changes(
        &self,
        changes: &crate::evm::EvmStateChanges,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        let batch = guard
            .as_mut()
            .ok_or("No active batch for b_apply_evm_state_changes")?;
        batch.apply_evm_changes(&changes.changes)
    }

    pub(super) fn b_register_evm_address(
        &self,
        evm_address: &[u8; 20],
        native: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.register_evm_address(evm_address, native)
        } else {
            self.state.register_evm_address(evm_address, native)
        }
    }

    pub(super) fn b_index_nft_mint(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_mint(collection, token, owner)
        } else {
            self.state.index_nft_mint(collection, token, owner)
        }
    }

    pub(super) fn b_index_nft_transfer(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_transfer(collection, token, from, to)
        } else {
            self.state.index_nft_transfer(collection, token, from, to)
        }
    }

    /// M6 fix: index NFT token_id through batch for atomicity.
    pub(super) fn b_index_nft_token_id(
        &self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_token_id(collection, token_id, token_account)
        } else {
            self.state
                .index_nft_token_id(collection, token_id, token_account)
        }
    }

    /// AUDIT-FIX 1.15: Check token_id uniqueness against batch overlay + committed state.
    pub(super) fn b_nft_token_id_exists(
        &self,
        collection: &Pubkey,
        token_id: u64,
    ) -> Result<bool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.nft_token_id_exists(collection, token_id)
        } else {
            self.state.nft_token_id_exists(collection, token_id)
        }
    }

    pub(super) fn b_index_program(&self, program: &Pubkey) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_program(program)
        } else {
            self.state.index_program(program)
        }
    }

    /// AUDIT-FIX HIGH-04: Validate symbol metadata before storage.
    pub(super) fn validate_and_sanitize_metadata(
        metadata: &Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let meta = match metadata {
            Some(value) => value,
            None => return Ok(None),
        };

        let obj = meta
            .as_object()
            .ok_or_else(|| "RegisterSymbol: metadata must be a JSON object".to_string())?;

        let mut clean = serde_json::Map::new();
        for (key, value) in obj {
            validate_symbol_registry_field_length(
                &format!("metadata key '{}'", key),
                key,
                MAX_SYMBOL_REGISTRY_METADATA_KEY_LEN,
            )?;
            if key.chars().any(|ch| ch.is_control()) {
                return Err(format!(
                    "RegisterSymbol: metadata key '{}' contains control characters",
                    key
                ));
            }
            let sanitized = match value {
                serde_json::Value::String(string) => {
                    let sanitized: String = string.chars().filter(|c| !c.is_control()).collect();
                    serde_json::Value::String(sanitized)
                }
                serde_json::Value::Number(_) | serde_json::Value::Bool(_) => value.clone(),
                _ => {
                    return Err(format!(
                        "RegisterSymbol: metadata value for '{}' must be a string, number, or boolean",
                        key
                    ));
                }
            };
            clean.insert(key.clone(), sanitized);
        }

        let serialized = serde_json::to_vec(&clean)
            .map_err(|e| format!("RegisterSymbol: metadata serialization failed: {}", e))?;
        if serialized.len() > 1024 {
            return Err(format!(
                "RegisterSymbol: metadata exceeds 1024 bytes ({})",
                serialized.len()
            ));
        }

        Ok(Some(serde_json::Value::Object(clean)))
    }

    pub(super) fn b_register_symbol(
        &self,
        symbol: &str,
        mut entry: SymbolRegistryEntry,
    ) -> Result<(), String> {
        entry.metadata = Self::validate_and_sanitize_metadata(&entry.metadata)?;
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.register_symbol(symbol, &entry)
        } else {
            self.state.register_symbol(symbol, entry)
        }
    }

    pub(super) fn b_next_governed_proposal_id(&self) -> Result<u64, String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.next_governed_proposal_id()
        } else {
            self.state.next_governed_proposal_id()
        }
    }

    pub(super) fn b_set_governed_proposal(
        &self,
        proposal: &crate::multisig::GovernedProposal,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.set_governed_proposal(proposal)
        } else {
            self.state.set_governed_proposal(proposal)
        }
    }

    pub(super) fn b_queue_governance_param_change(
        &self,
        param_id: u8,
        value: u64,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.queue_governance_param_change(param_id, value)
        } else {
            self.state.queue_governance_param_change(param_id, value)
        }
    }

    pub(super) fn b_next_governance_proposal_id(&self) -> Result<u64, String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.next_governance_proposal_id()
        } else {
            self.state.next_governance_proposal_id()
        }
    }

    pub(super) fn b_next_contract_deploy_nonce(&self, deployer: &Pubkey) -> Result<u64, String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.next_contract_deploy_nonce(deployer)
        } else {
            self.state.next_contract_deploy_nonce(deployer)
        }
    }

    pub(super) fn b_set_governance_proposal(
        &self,
        proposal: &GovernanceProposal,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.set_governance_proposal(proposal)
        } else {
            self.state.set_governance_proposal(proposal)
        }
    }

    pub(super) fn b_get_governance_proposal(
        &self,
        id: u64,
    ) -> Result<Option<GovernanceProposal>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_governance_proposal(id)
        } else {
            self.state.get_governance_proposal(id)
        }
    }

    pub(super) fn b_next_restriction_id(&self) -> Result<u64, String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.next_restriction_id()
        } else {
            self.state.next_restriction_id()
        }
    }

    pub(super) fn b_put_restriction(
        &self,
        record: &crate::restrictions::RestrictionRecord,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_restriction(record)
        } else {
            self.state.put_restriction(record)
        }
    }

    pub(super) fn b_get_restriction(
        &self,
        id: u64,
    ) -> Result<Option<crate::restrictions::RestrictionRecord>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_restriction(id)
        } else {
            self.state.get_restriction(id)
        }
    }

    pub(super) fn b_get_effective_restriction_record(
        &self,
        id: u64,
        slot: u64,
    ) -> Result<Option<crate::restrictions::EffectiveRestrictionRecord>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_effective_restriction_record(id, slot)
        } else {
            self.state.get_effective_restriction_record(id, slot)
        }
    }

    pub(super) fn b_get_active_restrictions_for_target(
        &self,
        target: &crate::restrictions::RestrictionTarget,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<crate::restrictions::RestrictionRecord>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_active_restrictions_for_target(target, slot, limit)
        } else {
            self.state
                .get_active_restrictions_for_target(target, slot, limit)
        }
    }

    pub(super) fn b_get_governed_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::multisig::GovernedProposal>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_governed_proposal(id)
        } else {
            self.state.get_governed_proposal(id)
        }
    }

    pub(super) fn b_get_last_slot(&self) -> Result<u64, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_last_slot()
        } else {
            self.state.get_last_slot()
        }
    }

    pub(super) fn b_get_governed_transfer_day_volume(
        &self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
    ) -> Result<u64, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_governed_transfer_day_volume(wallet_pubkey, day_start)
        } else {
            self.state
                .get_governed_transfer_day_volume(wallet_pubkey, day_start)
        }
    }

    pub(super) fn b_set_governed_transfer_day_volume(
        &self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
        volume: u64,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.set_governed_transfer_day_volume(wallet_pubkey, day_start, volume)
        } else {
            self.state
                .set_governed_transfer_day_volume(wallet_pubkey, day_start, volume)
        }
    }
}
