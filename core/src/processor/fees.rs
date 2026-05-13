use super::*;

impl TxProcessor {
    /// Calculate total fees for a transaction (base + program-specific + priority).
    ///
    /// Fee formula (Solana-inspired CU model):
    ///   `total = base_fee + instruction_premiums + priority_fee`
    /// where
    ///   `priority_fee = effective_compute_budget × compute_unit_price / 1_000_000`
    /// (compute_unit_price is in micro-spores per CU).
    ///
    /// All users pay the same base rate — reputation discounts removed (Task 4.2 M-7).
    pub fn compute_transaction_fee(tx: &Transaction, fee_config: &FeeConfig) -> u64 {
        let base_and_premiums = Self::compute_base_fee(tx, fee_config);
        let priority = Self::compute_priority_fee(tx);
        base_and_premiums.saturating_add(priority)
    }

    /// Compute only the base fee portion (base + instruction-specific premiums).
    /// Does NOT include priority fee.
    pub fn compute_base_fee(tx: &Transaction, fee_config: &FeeConfig) -> u64 {
        if let Some(first_ix) = tx.message.instructions.first() {
            if first_ix.program_id == SYSTEM_PROGRAM_ID {
                if let Some(&kind) = first_ix.data.first() {
                    if matches!(kind, 2..=5 | 19 | 26 | 27 | 30 | 31) {
                        return 0;
                    }
                }
            }
            if first_ix.program_id == EVM_PROGRAM_ID {
                if let Ok(evm_tx) = decode_evm_transaction(&first_ix.data) {
                    let estimated =
                        u256_to_spores(&(evm_tx.gas_price * U256::from(evm_tx.gas_limit)));
                    return if estimated > 0 {
                        estimated
                    } else {
                        fee_config.base_fee
                    };
                }
                return fee_config.base_fee;
            }
        }

        if !fee_config.fee_exempt_contracts.is_empty()
            && !tx.message.instructions.is_empty()
            && tx.message.instructions.iter().all(|ix| {
                ix.program_id == CONTRACT_PROGRAM_ID
                    && matches!(
                        ContractInstruction::deserialize(&ix.data),
                        Ok(ContractInstruction::Call { .. })
                    )
                    && ix.accounts.len() >= 2
                    && fee_config.fee_exempt_contracts.contains(&ix.accounts[1])
            })
        {
            return 0;
        }

        let mut total = fee_config.base_fee;

        for ix in &tx.message.instructions {
            if ix.program_id == SYSTEM_PROGRAM_ID {
                if let Some(kind) = ix.data.first() {
                    match *kind {
                        6 => total = total.saturating_add(fee_config.nft_collection_fee),
                        7 => total = total.saturating_add(fee_config.nft_mint_fee),
                        17 => total = total.saturating_add(fee_config.contract_deploy_fee),
                        #[cfg(feature = "zk")]
                        23 => total = total.saturating_add(crate::zk::SHIELD_COMPUTE_UNITS),
                        #[cfg(feature = "zk")]
                        24 => total = total.saturating_add(crate::zk::UNSHIELD_COMPUTE_UNITS),
                        #[cfg(feature = "zk")]
                        25 => total = total.saturating_add(crate::zk::TRANSFER_COMPUTE_UNITS),
                        _ => {}
                    }
                }
            }
            if ix.program_id == CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&ix.data) {
                    match contract_ix {
                        ContractInstruction::Deploy { .. } => {
                            total = total.saturating_add(fee_config.contract_deploy_fee)
                        }
                        ContractInstruction::Upgrade { .. }
                        | ContractInstruction::ExecuteUpgrade => {
                            total = total.saturating_add(fee_config.contract_upgrade_fee)
                        }
                        _ => {}
                    }
                }
            }
        }

        total
    }

    /// Compute the priority fee portion: `effective_compute_budget × compute_unit_price / 1_000_000`.
    pub fn compute_priority_fee(tx: &Transaction) -> u64 {
        let cu_price = tx.message.effective_compute_unit_price();
        if cu_price == 0 {
            return 0;
        }
        let budget = tx.message.effective_compute_budget();
        let product = budget as u128 * cu_price as u128;
        (product / 1_000_000).min(u64::MAX as u128) as u64
    }

    /// Derive the exact fee charged for a transaction from locally authoritative state.
    pub fn exact_transaction_fee_from_state(
        state: &StateStore,
        tx: &Transaction,
        fee_config: &FeeConfig,
    ) -> Option<u64> {
        let fallback_fee = Self::compute_transaction_fee(tx, fee_config);
        let first_ix = tx.message.instructions.first()?;

        if first_ix.program_id != EVM_PROGRAM_ID {
            return Some(fallback_fee);
        }

        let evm_tx = decode_evm_transaction(&first_ix.data).ok()?;
        let evm_hash: [u8; 32] = evm_tx.hash.into();
        let receipt = state.get_evm_receipt(&evm_hash).ok().flatten()?;
        let exact_fee = u256_to_spores(&(evm_tx.gas_price * U256::from(receipt.gas_used)));

        Some(if exact_fee > 0 {
            exact_fee
        } else {
            fallback_fee
        })
    }

    // AUDIT-FIX INFO-01: apply_reputation_fee_discount removed (was deprecated identity fn).
    // Previously: Task 4.2 (M-7) reputation-based fee discounts, always returned base_fee.

    /// Charge fee directly to state (not through batch), so it persists
    /// even if the instruction batch is later rolled back.
    pub(super) fn charge_fee_direct(&self, payer: &Pubkey, fee: u64) -> Result<(), String> {
        self.charge_fee_with_priority(payer, fee, 0)
    }

    /// Charge fee with separate base and priority fee handling.
    pub(super) fn charge_fee_with_priority(
        &self,
        payer: &Pubkey,
        total_fee: u64,
        priority_fee: u64,
    ) -> Result<(), String> {
        if self.is_speculative() {
            return self.charge_fee_with_priority_in_batch(payer, total_fee, priority_fee);
        }

        // Fee charging is a hidden shared-state update: every transaction debits
        // a payer and usually credits the treasury before instruction execution.
        // Keep this guard across the full read-modify-write, including the final
        // RocksDB batch write, so parallel TX groups cannot lose fee updates.
        let _fee_guard = self.state.lock_treasury()?;

        let mut payer_account = self
            .state
            .get_account(payer)?
            .ok_or_else(|| "Payer account not found".to_string())?;

        payer_account.deduct_spendable(total_fee)?;

        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let base_portion = total_fee.saturating_sub(priority_fee);
        let base_burn = (base_portion as u128 * fee_config.fee_burn_percent as u128 / 100) as u64;
        let base_producer =
            (base_portion as u128 * fee_config.fee_producer_percent as u128 / 100) as u64;
        let base_voters =
            (base_portion as u128 * fee_config.fee_voters_percent as u128 / 100) as u64;
        let base_community =
            (base_portion as u128 * fee_config.fee_community_percent as u128 / 100) as u64;
        let base_allocated = base_burn
            .saturating_add(base_producer)
            .saturating_add(base_voters)
            .saturating_add(base_community);
        let base_treasury = base_portion.saturating_sub(base_allocated);

        let priority_burn = priority_fee / 2;
        let priority_producer = priority_fee.saturating_sub(priority_burn);

        let burn_amount = base_burn.saturating_add(priority_burn);
        let total_to_treasury = base_treasury
            .saturating_add(base_producer)
            .saturating_add(base_voters)
            .saturating_add(base_community)
            .saturating_add(priority_producer);
        let capped_to_treasury =
            std::cmp::min(total_to_treasury, total_fee.saturating_sub(burn_amount));

        let mut accounts: Vec<(&Pubkey, &Account)> = vec![(payer, &payer_account)];
        let treasury_pubkey;
        let treasury_account;

        if capped_to_treasury > 0 {
            treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set".to_string())?;
            treasury_account = {
                let mut treasury = self
                    .state
                    .get_account(&treasury_pubkey)?
                    .unwrap_or_else(|| Account::new(0, treasury_pubkey));
                treasury.add_spendable(capped_to_treasury)?;
                treasury
            };
            accounts.push((&treasury_pubkey, &treasury_account));
        }

        self.state.atomic_put_accounts(&accounts, burn_amount)?;

        Ok(())
    }

    fn charge_fee_with_priority_in_batch(
        &self,
        payer: &Pubkey,
        total_fee: u64,
        priority_fee: u64,
    ) -> Result<(), String> {
        let mut payer_account = self
            .b_get_account(payer)?
            .ok_or_else(|| "Payer account not found".to_string())?;

        payer_account.deduct_spendable(total_fee)?;

        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let base_portion = total_fee.saturating_sub(priority_fee);
        let base_burn = (base_portion as u128 * fee_config.fee_burn_percent as u128 / 100) as u64;
        let base_producer =
            (base_portion as u128 * fee_config.fee_producer_percent as u128 / 100) as u64;
        let base_voters =
            (base_portion as u128 * fee_config.fee_voters_percent as u128 / 100) as u64;
        let base_community =
            (base_portion as u128 * fee_config.fee_community_percent as u128 / 100) as u64;
        let base_allocated = base_burn
            .saturating_add(base_producer)
            .saturating_add(base_voters)
            .saturating_add(base_community);
        let base_treasury = base_portion.saturating_sub(base_allocated);

        let priority_burn = priority_fee / 2;
        let priority_producer = priority_fee.saturating_sub(priority_burn);

        let burn_amount = base_burn.saturating_add(priority_burn);
        let total_to_treasury = base_treasury
            .saturating_add(base_producer)
            .saturating_add(base_voters)
            .saturating_add(base_community)
            .saturating_add(priority_producer);
        let capped_to_treasury =
            std::cmp::min(total_to_treasury, total_fee.saturating_sub(burn_amount));

        self.b_put_account(payer, &payer_account)?;

        if capped_to_treasury > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set".to_string())?;
            let mut treasury = self
                .b_get_account(&treasury_pubkey)?
                .unwrap_or_else(|| Account::new(0, treasury_pubkey));
            treasury.add_spendable(capped_to_treasury)?;
            self.b_put_account(&treasury_pubkey, &treasury)?;
        }

        self.b_add_burned(burn_amount)?;

        Ok(())
    }

    /// Compute the premium portion of a transaction fee (deploy/upgrade fees).
    pub(super) fn compute_premium_fee(tx: &Transaction, fee_config: &FeeConfig) -> u64 {
        let mut premium = 0u64;
        for ix in &tx.message.instructions {
            if ix.program_id == SYSTEM_PROGRAM_ID {
                if let Some(&kind) = ix.data.first() {
                    match kind {
                        6 => premium = premium.saturating_add(fee_config.nft_collection_fee),
                        7 => premium = premium.saturating_add(fee_config.nft_mint_fee),
                        17 => premium = premium.saturating_add(fee_config.contract_deploy_fee),
                        _ => {}
                    }
                }
            }
            if ix.program_id == CONTRACT_PROGRAM_ID {
                let data_str = std::str::from_utf8(&ix.data).unwrap_or("");
                if data_str.starts_with("{\"Deploy\"") {
                    premium = premium.saturating_add(fee_config.contract_deploy_fee);
                } else if data_str.starts_with("{\"Upgrade\"") {
                    premium = premium.saturating_add(fee_config.contract_upgrade_fee);
                }
            }
        }
        premium
    }

    /// Refund a premium fee amount to the payer account.
    pub(super) fn refund_premium(&self, payer: &Pubkey, amount: u64) -> Result<(), String> {
        if self.is_speculative() {
            let mut payer_account = self
                .b_get_account(payer)?
                .ok_or_else(|| "Payer account not found for refund".to_string())?;
            payer_account.add_spendable(amount)?;
            return self.b_put_account(payer, &payer_account);
        }

        let mut payer_account = self
            .state
            .get_account(payer)?
            .ok_or_else(|| "Payer account not found for refund".to_string())?;
        payer_account.add_spendable(amount)?;
        self.state.put_account(payer, &payer_account)
    }
}
