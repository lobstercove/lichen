use super::*;

impl TxProcessor {
    /// Start an atomic batch for the current transaction.
    fn begin_batch(&self) {
        *self.batch.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.state.begin_batch());
    }

    /// Commit the current batch atomically. Clears the active batch.
    fn commit_batch(&self) -> Result<(), String> {
        let batch = self
            .batch
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or_else(|| "No active batch to commit".to_string())?;
        self.state.commit_batch(batch)
    }

    /// Drop the current batch without committing (implicit rollback).
    fn rollback_batch(&self) {
        self.batch.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Process a transaction.
    pub fn process_transaction(&self, tx: &Transaction, _validator: &Pubkey) -> TxResult {
        self.process_transaction_inner(tx, _validator, None)
    }

    /// Process a transaction with optional pre-cached blockhashes.
    fn process_transaction_inner(
        &self,
        tx: &Transaction,
        _validator: &Pubkey,
        cached_blockhashes: Option<&HashSet<Hash>>,
    ) -> TxResult {
        {
            let mut meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
            meta.0 = None;
            meta.1.clear();
            meta.2 = 0;
        }

        if let Err(e) = tx.validate_structure() {
            return self.make_result(
                false,
                0,
                Some(format!("Invalid transaction structure: {}", e)),
                0,
            );
        }

        if tx.message.recent_blockhash == crate::hash::Hash::default() {
            return self.make_result(
                false,
                0,
                Some("Zero blockhash is not valid for replay protection".to_string()),
                0,
            );
        }

        let tx_hash = tx.hash();
        if let Ok(Some(_)) = self.state.get_transaction(&tx_hash) {
            return self.make_result(
                false,
                0,
                Some("Transaction already processed".to_string()),
                0,
            );
        }

        if tx.is_evm() {
            if is_evm_instruction(tx) {
                return self.process_evm_transaction(tx);
            } else {
                return self.make_result(
                    false,
                    0,
                    Some(
                        "EVM sentinel blockhash is reserved for EVM-wrapped transactions"
                            .to_string(),
                    ),
                    0,
                );
            }
        }

        {
            let valid = if let Some(hashes) = cached_blockhashes {
                hashes.contains(&tx.message.recent_blockhash)
            } else {
                let recent = self
                    .state
                    .get_recent_blockhashes(MAX_TX_AGE_BLOCKS)
                    .unwrap_or_default();
                recent.contains(&tx.message.recent_blockhash)
            };
            if !valid {
                let nonce_valid = Self::check_durable_nonce(tx, &self.state);
                if !nonce_valid {
                    return self.make_result(
                        false,
                        0,
                        Some("Blockhash not found or too old".to_string()),
                        0,
                    );
                }
            }
        }

        if is_evm_instruction(tx) {
            return self.process_evm_transaction(tx);
        }

        if let Err(error) = Self::verify_transaction_signatures(tx) {
            return self.make_result(false, 0, Some(error), 0);
        }

        let fee_payer = tx.message.instructions[0].accounts[0];
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());
        let base_fee = Self::compute_base_fee(tx, &fee_config);
        let priority_fee = Self::compute_priority_fee(tx);
        let total_fee = base_fee.saturating_add(priority_fee);
        let compute_budget = tx.message.effective_compute_budget();

        *self
            .tx_compute_budget
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = compute_budget;

        if total_fee > 0 {
            if let Err(e) = self.charge_fee_with_priority(&fee_payer, total_fee, priority_fee) {
                return self.make_result(false, 0, Some(format!("Fee error: {}", e)), 0);
            }
        }

        self.begin_batch();

        if let Err(e) = self.apply_rent(tx) {
            self.rollback_batch();
            if let Err(e2) = self.state.put_transaction(tx) {
                tracing::error!("Failed to store failed TX after rent error: {e2}");
            }
            if let Err(e2) = self.store_tx_meta(&tx.signature(), 0) {
                tracing::error!("Failed to store TX meta after rent error: {e2}");
            }
            return self.make_result(false, total_fee, Some(format!("Rent error: {}", e)), 0);
        }

        let native_cu = compute_units_for_tx(tx);
        if native_cu > compute_budget {
            self.rollback_batch();
            if let Err(e) = self.state.put_transaction(tx) {
                tracing::error!("Failed to store failed TX after CU budget exceeded: {e}");
            }
            if let Err(e) = self.store_tx_meta(&tx.signature(), native_cu) {
                tracing::error!("Failed to store TX meta after CU budget exceeded: {e}");
            }
            return self.make_result(
                false,
                total_fee,
                Some(format!(
                    "Compute budget exceeded: native instructions use {} CU, budget is {} CU",
                    native_cu, compute_budget
                )),
                native_cu,
            );
        }

        let mut total_cu = native_cu;
        for instruction in &tx.message.instructions {
            if let Err(e) = self.execute_instruction(instruction) {
                self.rollback_batch();

                let premium = Self::compute_premium_fee(tx, &fee_config);
                if premium > 0 {
                    if let Err(refund_err) = self.refund_premium(&fee_payer, premium) {
                        tracing::error!("Failed to refund deploy premium: {}", refund_err);
                    }
                }

                if let Err(e2) = self.state.put_transaction(tx) {
                    tracing::error!("Failed to store failed TX after execution error: {e2}");
                }
                if let Err(e2) = self.store_tx_meta(&tx.signature(), total_cu) {
                    tracing::error!("Failed to store TX meta after execution error: {e2}");
                }

                let actual_fee = total_fee.saturating_sub(premium);
                return self.make_result(
                    false,
                    actual_fee,
                    Some(format!("Execution error: {}", e)),
                    total_cu,
                );
            }

            if instruction.program_id == CONTRACT_PROGRAM_ID {
                let wasm_cu = {
                    let meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
                    meta.2
                };
                total_cu = native_cu.saturating_add(wasm_cu);

                if total_cu > compute_budget {
                    self.rollback_batch();
                    if let Err(e) = self.state.put_transaction(tx) {
                        tracing::error!("Failed to store failed TX after WASM CU exceeded: {e}");
                    }
                    if let Err(e) = self.store_tx_meta(&tx.signature(), total_cu) {
                        tracing::error!("Failed to store TX meta after WASM CU exceeded: {e}");
                    }
                    return self.make_result(
                        false,
                        total_fee,
                        Some(format!(
                            "Compute budget exceeded: used {} CU, budget is {} CU",
                            total_cu, compute_budget
                        )),
                        total_cu,
                    );
                }
            }
        }

        if Self::tx_updates_governance_fee_distribution(tx) {
            if let Err(e) = self.validate_pending_governance_fee_distribution() {
                self.rollback_batch();

                let premium = Self::compute_premium_fee(tx, &fee_config);
                if premium > 0 {
                    if let Err(refund_err) = self.refund_premium(&fee_payer, premium) {
                        tracing::error!("Failed to refund deploy premium: {}", refund_err);
                    }
                }

                if let Err(e2) = self.state.put_transaction(tx) {
                    tracing::error!(
                        "Failed to store failed TX after governance validation error: {e2}"
                    );
                }
                if let Err(e2) = self.store_tx_meta(&tx.signature(), total_cu) {
                    tracing::error!(
                        "Failed to store TX meta after governance validation error: {e2}"
                    );
                }

                let actual_fee = total_fee.saturating_sub(premium);
                return self.make_result(
                    false,
                    actual_fee,
                    Some(format!("Execution error: {}", e)),
                    total_cu,
                );
            }
        }

        if let Err(e) = self.detect_and_award_achievements(tx) {
            tracing::warn!("Achievement detection failed (non-fatal): {e}");
        }

        if let Err(e) = self.b_put_transaction(tx) {
            self.rollback_batch();
            let premium = Self::compute_premium_fee(tx, &fee_config);
            if premium > 0 {
                if let Err(refund_err) = self.refund_premium(&fee_payer, premium) {
                    tracing::error!("Failed to refund deploy premium: {}", refund_err);
                }
            }
            let actual_fee = total_fee.saturating_sub(premium);
            return self.make_result(
                false,
                actual_fee,
                Some(format!("Transaction storage error: {}", e)),
                total_cu,
            );
        }

        if let Err(e) = self.b_put_tx_meta(&tx.signature(), total_cu) {
            tracing::error!("Failed to store TX meta in commit batch: {e}");
        }

        if let Err(e) = self.commit_batch() {
            self.rollback_batch();
            let premium = Self::compute_premium_fee(tx, &fee_config);
            if premium > 0 {
                if let Err(refund_err) = self.refund_premium(&fee_payer, premium) {
                    tracing::error!("Failed to refund deploy premium: {}", refund_err);
                }
            }
            let actual_fee = total_fee.saturating_sub(premium);
            return self.make_result(
                false,
                actual_fee,
                Some(format!("Atomic commit failed: {}", e)),
                total_cu,
            );
        }

        self.make_result(true, total_fee, None, total_cu)
    }

    /// Process multiple transactions in parallel where possible.
    pub fn process_transactions_parallel(
        &self,
        txs: &[Transaction],
        validator: &Pubkey,
    ) -> Vec<TxResult> {
        let cached_blockhashes: HashSet<Hash> = self
            .state
            .get_recent_blockhashes(MAX_TX_AGE_BLOCKS)
            .unwrap_or_default()
            .into_iter()
            .collect();

        if txs.len() <= 1 {
            return txs
                .iter()
                .map(|tx| self.process_transaction_inner(tx, validator, Some(&cached_blockhashes)))
                .collect();
        }

        let n = txs.len();
        let tx_accounts: Vec<HashSet<Pubkey>> = txs
            .iter()
            .map(|tx| {
                let mut accounts = HashSet::new();
                for ix in &tx.message.instructions {
                    if ix.program_id != CONTRACT_PROGRAM_ID {
                        accounts.insert(ix.program_id);
                    }
                    for key in &ix.accounts {
                        accounts.insert(*key);
                    }
                    if ix.program_id == SYSTEM_PROGRAM_ID {
                        if let Some(&opcode) = ix.data.first() {
                            match opcode {
                                9 | 10 | 11 | 26 | 27 | 31 => {
                                    accounts.insert(CONFLICT_KEY_STAKE_POOL);
                                }
                                13..=16 => {
                                    accounts.insert(CONFLICT_KEY_MOSSSTAKE_POOL);
                                }
                                21 | 22 | 32 | 33 => {
                                    accounts.insert(CONFLICT_KEY_GOVERNED_PROPOSALS);
                                }
                                30 => {
                                    accounts.insert(CONFLICT_KEY_ORACLE);
                                }
                                34..=37 => {
                                    accounts.insert(CONFLICT_KEY_GOVERNANCE_PROPOSALS);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                accounts
            })
            .collect();

        let mut parent: Vec<usize> = (0..n).collect();

        fn uf_find(parent: &mut [usize], x: usize) -> usize {
            let mut root = x;
            while parent[root] != root {
                root = parent[root];
            }
            let mut current = x;
            while current != root {
                let next = parent[current];
                parent[current] = root;
                current = next;
            }
            root
        }

        fn uf_union(parent: &mut [usize], a: usize, b: usize) {
            let root_a = uf_find(parent, a);
            let root_b = uf_find(parent, b);
            if root_a != root_b {
                parent[root_a] = root_b;
            }
        }

        {
            let mut account_to_txs: std::collections::HashMap<Pubkey, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, accounts) in tx_accounts.iter().enumerate() {
                for account in accounts {
                    account_to_txs.entry(*account).or_default().push(i);
                }
            }
            for tx_indices in account_to_txs.values() {
                for window in tx_indices.windows(2) {
                    uf_union(&mut parent, window[0], window[1]);
                }
            }
        }

        let mut group_map: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            let root = uf_find(&mut parent, i);
            group_map.entry(root).or_default().push(i);
        }
        let groups: Vec<Vec<usize>> = group_map.into_values().collect();

        use rayon::prelude::*;

        let results_mu: std::sync::Mutex<Vec<TxResult>> = std::sync::Mutex::new(
            (0..n)
                .map(|_| TxResult {
                    success: false,
                    fee_paid: 0,
                    error: None,
                    compute_units_used: 0,
                    return_code: None,
                    contract_logs: Vec::new(),
                    return_data: Vec::new(),
                })
                .collect(),
        );

        groups.par_iter().for_each(|group| {
            let group_proc = TxProcessor::new(self.state.clone());
            let mut group_results: Vec<(usize, TxResult)> = Vec::with_capacity(group.len());
            for &idx in group {
                let result = group_proc.process_transaction_inner(
                    &txs[idx],
                    validator,
                    Some(&cached_blockhashes),
                );
                group_results.push((idx, result));
            }
            let mut results = results_mu.lock().unwrap_or_else(|e| e.into_inner());
            for (idx, result) in group_results {
                results[idx] = result;
            }
        });

        results_mu.into_inner().unwrap_or_else(|e| e.into_inner())
    }

    /// Simulate shielded instructions against the batch overlay without persisting.
    pub fn validate_shielded_preflight(&self, tx: &Transaction) -> Result<(), String> {
        let has_shielded = tx.message.instructions.iter().any(|instruction| {
            instruction.program_id == SYSTEM_PROGRAM_ID
                && matches!(instruction.data.first().copied(), Some(23..=25))
        });
        if !has_shielded {
            return Ok(());
        }

        self.begin_batch();
        let result =
            (|| {
                for instruction in &tx.message.instructions {
                    if instruction.program_id != SYSTEM_PROGRAM_ID {
                        continue;
                    }

                    match instruction.data.first().copied() {
                        Some(23) => {
                            #[cfg(feature = "zk")]
                            self.system_shield_deposit(instruction)?;
                            #[cfg(not(feature = "zk"))]
                            return Err("Shielded preflight requires the zk feature to be enabled"
                                .to_string());
                        }
                        Some(24) => {
                            #[cfg(feature = "zk")]
                            self.system_unshield_withdraw(instruction)?;
                            #[cfg(not(feature = "zk"))]
                            return Err("Shielded preflight requires the zk feature to be enabled"
                                .to_string());
                        }
                        Some(25) => {
                            #[cfg(feature = "zk")]
                            self.system_shielded_transfer(instruction)?;
                            #[cfg(not(feature = "zk"))]
                            return Err("Shielded preflight requires the zk feature to be enabled"
                                .to_string());
                        }
                        _ => {}
                    }
                }

                Ok(())
            })();
        self.rollback_batch();
        result
    }

    /// Simulate a transaction without persisting.
    pub fn simulate_transaction(&self, tx: &Transaction) -> SimulationResult {
        let mut logs = Vec::new();
        let mut last_return_code: Option<i64> = None;

        if tx.message.recent_blockhash == crate::hash::Hash::default() {
            return SimulationResult {
                success: false,
                fee: 0,
                logs,
                error: Some("Zero blockhash is not valid for replay protection".to_string()),
                compute_used: 0,
                return_data: None,
                return_code: None,
                state_changes: 0,
            };
        }

        if tx.is_evm() {
            if !is_evm_instruction(tx) {
                return SimulationResult {
                    success: false,
                    fee: 0,
                    logs,
                    error: Some(
                        "EVM sentinel blockhash is reserved for EVM-wrapped transactions"
                            .to_string(),
                    ),
                    compute_used: 0,
                    return_data: None,
                    return_code: None,
                    state_changes: 0,
                };
            }
        } else {
            let recent = self
                .state
                .get_recent_blockhashes(MAX_TX_AGE_BLOCKS)
                .unwrap_or_default();
            if !recent.contains(&tx.message.recent_blockhash)
                && !Self::check_durable_nonce(tx, &self.state)
            {
                return SimulationResult {
                    success: false,
                    fee: 0,
                    logs,
                    error: Some("Blockhash not found or too old".to_string()),
                    compute_used: 0,
                    return_data: None,
                    return_code: None,
                    state_changes: 0,
                };
            }
        }

        if tx.signatures.is_empty() || tx.message.instructions.is_empty() {
            return SimulationResult {
                success: false,
                fee: 0,
                logs,
                error: Some("Missing signatures or instructions".to_string()),
                compute_used: 0,
                return_data: None,
                return_code: None,
                state_changes: 0,
            };
        }

        if let Err(error) = Self::verify_transaction_signatures(tx) {
            return SimulationResult {
                success: false,
                fee: 0,
                logs,
                error: Some(error),
                compute_used: 0,
                return_data: None,
                return_code: None,
                state_changes: 0,
            };
        }

        let compute_budget = tx.message.effective_compute_budget();
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());
        let total_fee = Self::compute_transaction_fee(tx, &fee_config);
        let fee_payer = tx.message.instructions[0].accounts[0];
        let balance = self.state.get_balance(&fee_payer).unwrap_or(0);
        if balance < total_fee {
            return SimulationResult {
                success: false,
                fee: total_fee,
                logs,
                error: Some(format!(
                    "Insufficient balance for fee: need {} have {}",
                    total_fee, balance
                )),
                compute_used: 0,
                return_data: None,
                return_code: None,
                state_changes: 0,
            };
        }
        logs.push(format!(
            "Fee estimate: {} spores (budget: {} CU)",
            total_fee, compute_budget
        ));

        let mut total_compute = 0u64;
        let mut last_return_data: Option<Vec<u8>> = None;
        let mut total_state_changes: usize = 0;

        for (idx, instruction) in tx.message.instructions.iter().enumerate() {
            if instruction.program_id == CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&instruction.data) {
                    match contract_ix {
                        ContractInstruction::Call {
                            function,
                            args,
                            value,
                        } => {
                            if instruction.accounts.len() >= 2 {
                                let caller = &instruction.accounts[0];
                                let contract_addr = &instruction.accounts[1];

                                match self.state.get_account(contract_addr) {
                                    Ok(Some(account)) if account.executable => {
                                        if let Ok(contract) =
                                            serde_json::from_slice::<ContractAccount>(&account.data)
                                        {
                                            let current_slot =
                                                self.state.get_last_slot().unwrap_or(0);
                                            let live_storage = self
                                                .state
                                                .load_contract_storage_map(contract_addr)
                                                .unwrap_or_default()
                                                .into_iter()
                                                .collect();
                                            let remaining =
                                                compute_budget.saturating_sub(total_compute);
                                            let context = build_top_level_call_context(
                                                ContractContext::with_args(
                                                    *caller,
                                                    *contract_addr,
                                                    value,
                                                    current_slot,
                                                    live_storage,
                                                    args.clone(),
                                                ),
                                                self.state.clone(),
                                                remaining,
                                            );
                                            let mut runtime = ContractRuntime::get_pooled();
                                            let exec_result = runtime
                                                .execute(&contract, &function, &args, context);
                                            runtime.return_to_pool();
                                            match exec_result {
                                                Ok(result) => {
                                                    let cross_call_state_changes = result
                                                        .cross_call_changes
                                                        .values()
                                                        .map(HashMap::len)
                                                        .sum::<usize>();
                                                    let instruction_state_changes =
                                                        result.storage_changes.len()
                                                            + cross_call_state_changes;
                                                    total_compute += result.compute_used;
                                                    last_return_code = result.return_code;
                                                    total_state_changes +=
                                                        instruction_state_changes;
                                                    for log in result
                                                        .logs
                                                        .iter()
                                                        .chain(result.cross_call_logs.iter())
                                                    {
                                                        logs.push(format!("[ix{}] {}", idx, log));
                                                    }
                                                    if !result.return_data.is_empty() {
                                                        last_return_data =
                                                            Some(result.return_data.clone());
                                                    }
                                                    if !result.success {
                                                        return SimulationResult {
                                                            success: false,
                                                            fee: total_fee,
                                                            logs,
                                                            error: result.error,
                                                            compute_used: total_compute,
                                                            return_data: last_return_data,
                                                            return_code: last_return_code,
                                                            state_changes: total_state_changes,
                                                        };
                                                    }
                                                    if let Some(rc) = result.return_code {
                                                        let meaningful_changes = result
                                                            .storage_changes
                                                            .keys()
                                                            .any(|k| !k.ends_with(b"_reentrancy"));
                                                        if rc != 0
                                                            && !meaningful_changes
                                                            && result.cross_call_changes.is_empty()
                                                        {
                                                            logs.push(format!(
                                                                "[ix{}] Contract '{}' returned error code {} with no state changes",
                                                                idx, function, rc
                                                            ));
                                                            return SimulationResult {
                                                                success: false,
                                                                fee: total_fee,
                                                                logs,
                                                                error: Some(format!(
                                                                    "Contract '{}' returned error code {} with no state changes",
                                                                    function, rc
                                                                )),
                                                                compute_used: total_compute,
                                                                return_data: last_return_data,
                                                                return_code: last_return_code,
                                                                state_changes: total_state_changes,
                                                            };
                                                        }
                                                    }
                                                    logs.push(format!(
                                                        "[ix{}] Contract call '{}' OK, compute: {}, changes: {}",
                                                        idx, function, result.compute_used, instruction_state_changes
                                                    ));
                                                    if total_compute > compute_budget {
                                                        return SimulationResult {
                                                            success: false,
                                                            fee: total_fee,
                                                            logs,
                                                            error: Some(format!(
                                                                "Compute budget exceeded: used {} CU, budget is {} CU",
                                                                total_compute, compute_budget
                                                            )),
                                                            compute_used: total_compute,
                                                            return_data: last_return_data,
                                                            return_code: last_return_code,
                                                            state_changes: total_state_changes,
                                                        };
                                                    }
                                                }
                                                Err(e) => {
                                                    return SimulationResult {
                                                        success: false,
                                                        fee: total_fee,
                                                        logs,
                                                        error: Some(format!(
                                                            "Contract execution error: {}",
                                                            e
                                                        )),
                                                        compute_used: total_compute,
                                                        return_data: last_return_data,
                                                        return_code: last_return_code,
                                                        state_changes: total_state_changes,
                                                    };
                                                }
                                            }
                                        }
                                    }
                                    Ok(Some(_)) => {
                                        logs.push(format!("[ix{}] Account is not executable", idx));
                                    }
                                    _ => {
                                        logs.push(format!("[ix{}] Contract not found", idx));
                                    }
                                }
                            }
                        }
                        ContractInstruction::Deploy { .. } => {
                            logs.push(format!(
                                "[ix{}] Deploy instruction (would deploy contract)",
                                idx
                            ));
                        }
                        ContractInstruction::Upgrade { .. } => {
                            logs.push(format!(
                                "[ix{}] Upgrade instruction (would upgrade contract)",
                                idx
                            ));
                        }
                        ContractInstruction::Close => {
                            logs.push(format!(
                                "[ix{}] Close instruction (would close contract)",
                                idx
                            ));
                        }
                        ContractInstruction::SetUpgradeTimelock { epochs } => {
                            logs.push(format!(
                                "[ix{}] SetUpgradeTimelock instruction (epochs={})",
                                idx, epochs
                            ));
                        }
                        ContractInstruction::ExecuteUpgrade => {
                            logs.push(format!(
                                "[ix{}] ExecuteUpgrade instruction (would apply staged upgrade)",
                                idx
                            ));
                        }
                        ContractInstruction::VetoUpgrade => {
                            logs.push(format!(
                                "[ix{}] VetoUpgrade instruction (would cancel pending upgrade)",
                                idx
                            ));
                        }
                    }
                }
            } else if instruction.program_id == SYSTEM_PROGRAM_ID {
                let cu = instruction
                    .data
                    .first()
                    .map(|&t| compute_units_for_system_ix(t))
                    .unwrap_or(0);
                total_compute += cu;
                logs.push(format!("[ix{}] System instruction ({} CU)", idx, cu));
                if total_compute > compute_budget {
                    return SimulationResult {
                        success: false,
                        fee: total_fee,
                        logs,
                        error: Some(format!(
                            "Compute budget exceeded: used {} CU, budget is {} CU",
                            total_compute, compute_budget
                        )),
                        compute_used: total_compute,
                        return_data: last_return_data,
                        return_code: last_return_code,
                        state_changes: total_state_changes,
                    };
                }
            } else if instruction.program_id == EVM_PROGRAM_ID {
                logs.push(format!(
                    "[ix{}] EVM instruction (use eth_call for simulation)",
                    idx
                ));
            } else {
                logs.push(format!(
                    "[ix{}] Unknown program: {}",
                    idx, instruction.program_id
                ));
            }
        }

        SimulationResult {
            success: true,
            fee: total_fee,
            logs,
            error: None,
            compute_used: total_compute,
            return_data: last_return_data,
            return_code: last_return_code,
            state_changes: total_state_changes,
        }
    }

    /// Process an EVM transaction.
    fn process_evm_transaction(&self, tx: &Transaction) -> TxResult {
        if tx.message.instructions.len() != 1 {
            return self.make_result(
                false,
                0,
                Some("Invalid EVM transaction format".to_string()),
                0,
            );
        }

        let instruction = &tx.message.instructions[0];
        let raw = &instruction.data;

        let evm_tx = match decode_evm_transaction(raw) {
            Ok(tx) => tx,
            Err(err) => {
                return self.make_result(false, 0, Some(err), 0);
            }
        };

        if !u256_is_multiple_of_spore(&evm_tx.value) {
            return self.make_result(
                false,
                0,
                Some("EVM value must be multiple of 1e9 wei".to_string()),
                0,
            );
        }

        let from_address: [u8; 20] = evm_tx.from.into();
        let mapping = match self.state.lookup_evm_address(&from_address) {
            Ok(value) => value,
            Err(err) => {
                return self.make_result(false, 0, Some(err), 0);
            }
        };

        if mapping.is_none() {
            return self.make_result(false, 0, Some("EVM address not registered".to_string()), 0);
        }

        let chain_id = evm_tx.chain_id.unwrap_or(0);
        let (result, evm_state_changes) =
            match execute_evm_transaction(self.state.clone(), &evm_tx, chain_id) {
                Ok(res) => res,
                Err(err) => {
                    return self.make_result(false, 0, Some(err), 0);
                }
            };

        let evm_hash: [u8; 32] = evm_tx.hash.into();
        let native_hash = tx.hash().0;

        let record = EvmTxRecord {
            evm_hash,
            native_hash,
            from: from_address,
            to: evm_tx.to.map(|addr| addr.into()),
            value: evm_tx.value.to_be_bytes(),
            gas_limit: evm_tx.gas_limit,
            gas_price: evm_tx.gas_price.to_be_bytes(),
            nonce: evm_tx.nonce,
            data: evm_tx.data.to_vec(),
            status: Some(result.success),
            gas_used: Some(result.gas_used),
            block_slot: None,
            block_hash: None,
        };

        let receipt = EvmReceipt {
            evm_hash,
            status: result.success,
            gas_used: result.gas_used,
            block_slot: None,
            block_hash: None,
            contract_address: result.created_address,
            logs: result.logs.clone(),
            structured_logs: result.structured_logs.clone(),
        };

        let evm_log_entries: Vec<crate::evm::EvmLogEntry> = result
            .structured_logs
            .iter()
            .enumerate()
            .map(|(i, log)| crate::evm::EvmLogEntry {
                tx_hash: evm_hash,
                tx_index: 0,
                log_index: i as u32,
                log: log.clone(),
            })
            .collect();

        let fee_paid = u256_to_spores(&(evm_tx.gas_price * U256::from(result.gas_used)));
        if fee_paid > 0 {
            let native_payer = match mapping {
                Some(payer) => payer,
                None => {
                    return self.make_result(
                        false,
                        0,
                        Some("EVM fee charge error: missing native payer mapping".to_string()),
                        0,
                    );
                }
            };
            if let Err(e) = self.charge_fee_direct(&native_payer, fee_paid) {
                return self.make_result(false, 0, Some(format!("EVM fee charge error: {}", e)), 0);
            }
        }

        self.begin_batch();

        if let Err(e) = self.b_put_evm_tx(&record) {
            self.rollback_batch();
            return self.make_result(
                false,
                fee_paid,
                Some(format!("EVM tx storage error: {}", e)),
                0,
            );
        }
        if let Err(e) = self.b_put_evm_receipt(&receipt) {
            self.rollback_batch();
            return self.make_result(
                false,
                fee_paid,
                Some(format!("EVM receipt storage error: {}", e)),
                0,
            );
        }

        if !evm_log_entries.is_empty() {
            let slot = self.state.get_last_slot().unwrap_or(0);
            if let Err(e) = self.b_put_evm_logs_for_slot(slot, &evm_log_entries) {
                self.rollback_batch();
                return self.make_result(
                    false,
                    fee_paid,
                    Some(format!("EVM log index error: {}", e)),
                    0,
                );
            }
        }

        if let Err(e) = self.b_put_transaction(tx) {
            self.rollback_batch();
            return self.make_result(
                false,
                fee_paid,
                Some(format!("Transaction storage error: {}", e)),
                0,
            );
        }

        if let Err(e) = self.b_apply_evm_state_changes(&evm_state_changes) {
            self.rollback_batch();
            return self.make_result(
                false,
                fee_paid,
                Some(format!("EVM state apply error: {}", e)),
                0,
            );
        }

        if let Err(e) = self.commit_batch() {
            self.rollback_batch();
            return self.make_result(
                false,
                fee_paid,
                Some(format!("Atomic commit failed: {}", e)),
                0,
            );
        }

        self.make_result(
            result.success,
            fee_paid,
            if result.success {
                None
            } else {
                Some("EVM execution reverted".to_string())
            },
            0,
        )
    }
}
