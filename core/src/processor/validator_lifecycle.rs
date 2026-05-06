use super::*;
use crate::restrictions::{ProtocolModuleId, RestrictionTransferDirection};

impl TxProcessor {
    /// On-chain validator registration with bootstrap grant (instruction type 26).
    /// Processes validator admission through consensus so ALL nodes see identical state.
    ///
    /// Instruction data: [26 | machine_fingerprint(32)]
    /// Accounts: [new_validator_pubkey]
    ///
    /// This is fee-exempt because the new validator has no account yet.
    /// The treasury funds the bootstrap grant (100K LICN) which is immediately staked.
    pub(super) fn system_register_validator(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("RegisterValidator requires [validator] account".to_string());
        }
        if ix.data.len() < 33 {
            return Err(
                "RegisterValidator: missing machine_fingerprint (need 33 bytes)".to_string(),
            );
        }

        let validator_pubkey = ix.accounts[0];
        let mut fingerprint = [0u8; 32];
        fingerprint.copy_from_slice(&ix.data[1..33]);

        if let Some(existing) = self.b_get_account(&validator_pubkey)? {
            if existing.staked >= crate::consensus::BOOTSTRAP_GRANT_AMOUNT {
                return Ok(());
            }
        }

        let pool = self.b_get_stake_pool()?;
        let grants_issued = pool.bootstrap_grants_issued();
        if grants_issued >= crate::consensus::MAX_BOOTSTRAP_VALIDATORS {
            return Err(format!(
                "RegisterValidator: bootstrap phase complete ({} grants issued, max {})",
                grants_issued,
                crate::consensus::MAX_BOOTSTRAP_VALIDATORS
            ));
        }

        if fingerprint != [0u8; 32] {
            if let Some(existing_pk) = pool.fingerprint_owner(&fingerprint) {
                if existing_pk != &validator_pubkey {
                    return Err(format!(
                        "RegisterValidator: machine fingerprint already registered to {}",
                        existing_pk.to_base58()
                    ));
                }
            }
        }
        drop(pool);

        self.ensure_protocol_module_not_paused(ProtocolModuleId::Staking, "RegisterValidator")?;

        let treasury_pubkey = self
            .state
            .get_treasury_pubkey()?
            .ok_or_else(|| "RegisterValidator: treasury pubkey not set".to_string())?;
        let mut treasury = self
            .b_get_account(&treasury_pubkey)?
            .ok_or_else(|| "RegisterValidator: treasury account not found".to_string())?;

        let grant_amount = crate::consensus::BOOTSTRAP_GRANT_AMOUNT;
        self.ensure_native_account_direction_not_restricted(
            &treasury_pubkey,
            RestrictionTransferDirection::Outgoing,
            grant_amount,
            treasury.spendable,
            "RegisterValidator",
            "treasury",
        )?;
        let mut account = self
            .b_get_account(&validator_pubkey)?
            .unwrap_or_else(|| Account {
                spores: 0,
                spendable: 0,
                staked: 0,
                locked: 0,
                data: Vec::new(),
                public_key: None,
                owner: Pubkey([0x01; 32]),
                executable: false,
                rent_epoch: 0,
                dormant: false,
                missed_rent_epochs: 0,
            });
        self.ensure_native_account_direction_not_restricted(
            &validator_pubkey,
            RestrictionTransferDirection::Incoming,
            grant_amount,
            account.spendable,
            "RegisterValidator",
            "validator",
        )?;

        treasury
            .deduct_spendable(grant_amount)
            .map_err(|e| format!("RegisterValidator: treasury insufficient: {}", e))?;
        self.b_put_account(&treasury_pubkey, &treasury)?;

        account.spores = account.spores.saturating_add(grant_amount);
        account.staked = account.staked.saturating_add(grant_amount);
        self.b_put_account(&validator_pubkey, &account)?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_stake_pool()?;
        pool.try_bootstrap_with_fingerprint(
            validator_pubkey,
            grant_amount,
            current_slot,
            fingerprint,
        )
        .map_err(|e| format!("RegisterValidator: stake pool error: {}", e))?;
        self.b_put_stake_pool(&pool)?;

        Ok(())
    }

    /// System program: SlashValidator (opcode 27)
    ///
    /// Consensus-based equivocation slashing — the Ethereum/Cosmos pattern.
    /// Any validator that detects a DoubleVote or DoubleBlock creates this
    /// transaction with the cryptographic evidence. When the transaction is
    /// included in a block, ALL validators verify the evidence and apply the
    /// same economic penalty deterministically.
    pub(super) fn system_slash_validator(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("SlashValidator requires [offending_validator] account".to_string());
        }
        if ix.data.len() < 2 {
            return Err("SlashValidator: missing evidence data".to_string());
        }

        let offending_validator = ix.accounts[0];

        let evidence: crate::consensus::SlashingEvidence = bincode::deserialize(&ix.data[1..])
            .map_err(|e| format!("SlashValidator: invalid evidence encoding: {}", e))?;

        if evidence.validator != offending_validator {
            return Err(format!(
                "SlashValidator: evidence validator {} doesn't match account {}",
                evidence.validator.to_base58(),
                offending_validator.to_base58()
            ));
        }

        match &evidence.offense {
            crate::consensus::SlashingOffense::DoubleVote {
                slot: _,
                vote_1,
                vote_2,
            } => {
                if vote_1.validator != offending_validator
                    || vote_2.validator != offending_validator
                {
                    return Err("SlashValidator: vote signers don't match offender".to_string());
                }
                if vote_1.slot != vote_2.slot {
                    return Err("SlashValidator: votes are for different slots".to_string());
                }
                if vote_1.block_hash == vote_2.block_hash {
                    return Err("SlashValidator: votes are for the same block".to_string());
                }
                if !vote_1.verify() || !vote_2.verify() {
                    return Err(
                        "SlashValidator: one or both vote signatures are invalid".to_string()
                    );
                }
            }
            crate::consensus::SlashingOffense::DoubleBlock {
                slot: _,
                block_hash_1,
                block_hash_2,
            } => {
                if block_hash_1 == block_hash_2 {
                    return Err("SlashValidator: block hashes are identical".to_string());
                }
            }
            _ => {
                return Err(
                    "SlashValidator: only DoubleVote and DoubleBlock are consensus-slashable"
                        .to_string(),
                );
            }
        }

        let offense_key = match &evidence.offense {
            crate::consensus::SlashingOffense::DoubleVote { slot, .. } => {
                format!(
                    "slashed:{}:{}:double_vote",
                    offending_validator.to_base58(),
                    slot
                )
            }
            crate::consensus::SlashingOffense::DoubleBlock { slot, .. } => {
                format!(
                    "slashed:{}:{}:double_block",
                    offending_validator.to_base58(),
                    slot
                )
            }
            _ => unreachable!(),
        };
        if self
            .state
            .get_metadata(&offense_key)
            .ok()
            .flatten()
            .is_some()
        {
            return Ok(());
        }

        let params = crate::genesis::ConsensusParams::default();
        let mut pool = self.b_get_stake_pool()?;
        let original_stake = pool
            .get_stake(&offending_validator)
            .map(|s| s.total_stake())
            .unwrap_or(0);

        if original_stake == 0 {
            self.state.put_metadata(&offense_key, b"1").map_err(|e| {
                format!(
                    "SlashValidator: failed to persist idempotency marker: {}",
                    e
                )
            })?;
            return Ok(());
        }

        let slash_percent = match &evidence.offense {
            crate::consensus::SlashingOffense::DoubleVote { .. } => {
                params.slashing_percentage_double_vote
            }
            crate::consensus::SlashingOffense::DoubleBlock { .. } => {
                params.slashing_percentage_double_sign
            }
            _ => unreachable!(),
        };

        let raw_penalty = (original_stake as u128 * slash_percent as u128 / 100) as u64;
        let slash_budget = original_stake.saturating_sub(crate::consensus::MIN_VALIDATOR_STAKE);
        let capped_penalty = raw_penalty.min(slash_budget);

        if capped_penalty > 0 {
            pool.slash_validator(&offending_validator, capped_penalty);
            self.b_put_stake_pool(&pool)?;

            if let Some(mut acct) = self.b_get_account(&offending_validator)? {
                let debit = capped_penalty.min(acct.staked);
                acct.staked = acct.staked.saturating_sub(debit);
                acct.spores = acct.spores.saturating_sub(debit);
                self.b_put_account(&offending_validator, &acct)?;
            }

            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "SlashValidator: treasury pubkey not set".to_string())?;
            if let Some(mut treasury) = self.b_get_account(&treasury_pubkey)? {
                treasury.spores = treasury.spores.saturating_add(capped_penalty);
                treasury.spendable = treasury.spendable.saturating_add(capped_penalty);
                self.b_put_account(&treasury_pubkey, &treasury)?;
            }
        }

        self.state.put_metadata(&offense_key, b"1").map_err(|e| {
            format!(
                "SlashValidator: failed to persist idempotency marker: {}",
                e
            )
        })?;

        Ok(())
    }

    /// System program: DeregisterValidator (opcode 31).
    ///
    /// Voluntary validator exit following the Ethereum beacon chain pattern.
    /// The validator signals intent to leave; actual removal happens at the next
    /// epoch boundary.
    pub(super) fn system_deregister_validator(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("DeregisterValidator requires [validator] account".to_string());
        }

        let validator_pubkey = ix.accounts[0];

        let mut pool = self.b_get_stake_pool()?;
        let stake_info = pool
            .get_stake(&validator_pubkey)
            .ok_or_else(|| {
                format!(
                    "DeregisterValidator: validator {} not found in stake pool",
                    validator_pubkey.to_base58()
                )
            })?
            .clone();

        if !stake_info.is_active {
            return Ok(());
        }

        self.ensure_protocol_module_not_paused(ProtocolModuleId::Staking, "DeregisterValidator")?;

        if let Some(si) = pool.get_stake_mut(&validator_pubkey) {
            si.is_active = false;
        }
        self.b_put_stake_pool(&pool)?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let current_epoch = crate::consensus::slot_to_epoch(current_slot);
        let change = crate::consensus::PendingValidatorChange {
            pubkey: validator_pubkey,
            change_type: crate::consensus::ValidatorChangeType::Remove,
            queued_at_slot: current_slot,
            effective_epoch: current_epoch + 1,
        };
        self.state
            .queue_pending_validator_change(&change)
            .map_err(|e| format!("DeregisterValidator: failed to queue pending change: {}", e))?;

        Ok(())
    }
}
