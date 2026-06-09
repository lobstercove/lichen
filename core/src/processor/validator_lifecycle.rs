use super::*;
use crate::codec::deserialize_legacy_bincode_strict;
use crate::consensus::SLASHING_EVIDENCE_CODEC_LIMIT_BYTES;
use crate::restrictions::{ProtocolModuleId, RestrictionTransferDirection};

const REGISTER_VALIDATOR_LEGACY_LEN: usize = 33;
const REGISTER_VALIDATOR_EXPLICIT_GRANT_LEN: usize = 34;
const REGISTER_VALIDATOR_SELF_FUNDED_LEN: usize = 42;
const REGISTER_VALIDATOR_MODE_OFFSET: usize = 33;
const REGISTER_VALIDATOR_MODE_GRANT: u8 = 0;
const REGISTER_VALIDATOR_MODE_SELF_FUNDED: u8 = 1;
// The original lichen-testnet-1 policy is bootstrap-grant admission for the
// first MAX_BOOTSTRAP_VALIDATORS validators. Later local genesis configs may
// have persisted the metadata flag as disabled, and that flag is not a reliable
// consensus policy source for historical or live testnet replay. Derive the
// testnet grant policy from immutable chain identity; the stake pool's grant
// counter is the actual cap.
const TESTNET_BOOTSTRAP_GRANTS_CHAIN_ID: &str = "lichen-testnet-1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidatorRegistrationMode {
    BootstrapGrant,
    SelfFunded { amount: u64 },
}

fn decode_validator_registration_mode(data: &[u8]) -> Result<ValidatorRegistrationMode, String> {
    match data.len() {
        REGISTER_VALIDATOR_LEGACY_LEN => Ok(ValidatorRegistrationMode::BootstrapGrant),
        REGISTER_VALIDATOR_EXPLICIT_GRANT_LEN => {
            if data[REGISTER_VALIDATOR_MODE_OFFSET] == REGISTER_VALIDATOR_MODE_GRANT {
                Ok(ValidatorRegistrationMode::BootstrapGrant)
            } else {
                Err("RegisterValidator: invalid registration mode".to_string())
            }
        }
        REGISTER_VALIDATOR_SELF_FUNDED_LEN => match data[REGISTER_VALIDATOR_MODE_OFFSET] {
            REGISTER_VALIDATOR_MODE_SELF_FUNDED => {
                let amount_bytes: [u8; 8] = data[34..42]
                    .try_into()
                    .map_err(|_| "RegisterValidator: invalid self-funded amount".to_string())?;
                let amount = u64::from_le_bytes(amount_bytes);
                if amount == 0 {
                    return Err("RegisterValidator: self-funded amount must be nonzero".to_string());
                }
                Ok(ValidatorRegistrationMode::SelfFunded { amount })
            }
            REGISTER_VALIDATOR_MODE_GRANT => Err(
                "RegisterValidator: grant mode must not include trailing self-funded amount"
                    .to_string(),
            ),
            _ => Err("RegisterValidator: invalid registration mode".to_string()),
        },
        len if len < REGISTER_VALIDATOR_LEGACY_LEN => {
            Err("RegisterValidator: missing machine_fingerprint (need 33 bytes)".to_string())
        }
        _ => Err("RegisterValidator: invalid instruction length".to_string()),
    }
}

impl TxProcessor {
    /// On-chain validator registration with bootstrap grant (instruction type 26).
    /// Processes validator admission through consensus so ALL nodes see identical state.
    ///
    /// Instruction data:
    /// - legacy/dev grant: [26 | machine_fingerprint(32)]
    /// - explicit dev grant: [26 | machine_fingerprint(32) | 0]
    /// - self-funded: [26 | machine_fingerprint(32) | 1 | amount_u64_le]
    ///   Accounts: [new_validator_pubkey]
    ///
    /// Treasury bootstrap grants are enabled when chain policy permits them.
    /// On lichen-testnet-1, the first MAX_BOOTSTRAP_VALIDATORS validators use
    /// the same bootstrap-recovery schedule as the genesis validators.
    pub(super) fn system_register_validator(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("RegisterValidator requires [validator] account".to_string());
        }
        let mode = decode_validator_registration_mode(&ix.data)?;

        let validator_pubkey = ix.accounts[0];
        let mut fingerprint = [0u8; 32];
        fingerprint.copy_from_slice(&ix.data[1..33]);

        if fingerprint == [0u8; 32] {
            return Err(
                "RegisterValidator: zero machine fingerprint is not accepted for validator registration"
                    .to_string(),
            );
        }

        match mode {
            ValidatorRegistrationMode::BootstrapGrant => {
                self.system_register_validator_bootstrap_grant(validator_pubkey, fingerprint)
            }
            ValidatorRegistrationMode::SelfFunded { amount } => {
                self.system_register_validator_self_funded(validator_pubkey, fingerprint, amount)
            }
        }
    }

    fn validator_bootstrap_grants_enabled(&self) -> Result<bool, String> {
        let metadata_enabled = self
            .state
            .get_metadata(crate::consensus::VALIDATOR_BOOTSTRAP_GRANTS_ENABLED_METADATA_KEY)?
            .as_deref()
            == Some(crate::consensus::VALIDATOR_BOOTSTRAP_GRANTS_ENABLED_VALUE);
        if metadata_enabled {
            return Ok(true);
        }

        self.testnet_bootstrap_grants_enabled()
    }

    fn testnet_bootstrap_grants_enabled(&self) -> Result<bool, String> {
        let Some(chain_id_bytes) = self
            .state
            .get_metadata(crate::signing::CHAIN_ID_METADATA_KEY)?
        else {
            return Ok(false);
        };
        let Ok(chain_id) = std::str::from_utf8(&chain_id_bytes) else {
            return Ok(false);
        };
        if chain_id != TESTNET_BOOTSTRAP_GRANTS_CHAIN_ID {
            return Ok(false);
        }

        Ok(true)
    }

    fn system_register_validator_bootstrap_grant(
        &self,
        validator_pubkey: Pubkey,
        fingerprint: [u8; 32],
    ) -> Result<(), String> {
        if !self.validator_bootstrap_grants_enabled()? {
            return Err(
                "RegisterValidator: treasury bootstrap grants are disabled on this chain"
                    .to_string(),
            );
        }

        let mut pool = self.b_get_stake_pool()?;
        if let Some(existing_pk) = pool.fingerprint_owner(&fingerprint) {
            if existing_pk != &validator_pubkey {
                return Err(format!(
                    "RegisterValidator: machine fingerprint already registered to {}",
                    existing_pk.to_base58()
                ));
            }
        }

        if let Some(existing) = self.b_get_account(&validator_pubkey)? {
            if existing.staked >= crate::consensus::BOOTSTRAP_GRANT_AMOUNT {
                if pool
                    .get_stake(&validator_pubkey)
                    .map(|stake| stake.total_stake() >= crate::consensus::BOOTSTRAP_GRANT_AMOUNT)
                    .unwrap_or(false)
                {
                    pool.register_fingerprint(&validator_pubkey, fingerprint)
                        .map_err(|e| format!("RegisterValidator: stake pool error: {}", e))?;
                    self.b_put_stake_pool(&pool)?;
                    return Ok(());
                }
                return Err(
                    "RegisterValidator: existing staked account is not backed by stake-pool registration"
                        .to_string(),
                );
            }
            if existing.staked > 0 {
                return Err(
                    "RegisterValidator: existing validator account has partial stake; complete stake-pool registration through an explicit funded path or repair the stake-pool state"
                        .to_string(),
                );
            }
        }
        let grants_issued = pool.bootstrap_grants_issued();
        if grants_issued >= crate::consensus::MAX_BOOTSTRAP_VALIDATORS {
            return Err(format!(
                "RegisterValidator: bootstrap phase complete ({} grants issued, max {})",
                grants_issued,
                crate::consensus::MAX_BOOTSTRAP_VALIDATORS
            ));
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

    fn system_register_validator_self_funded(
        &self,
        validator_pubkey: Pubkey,
        fingerprint: [u8; 32],
        amount: u64,
    ) -> Result<(), String> {
        self.ensure_protocol_module_not_paused(ProtocolModuleId::Staking, "RegisterValidator")?;

        let mut pool = self.b_get_stake_pool()?;
        if pool.get_stake(&validator_pubkey).is_some() {
            return Err(
                "RegisterValidator: validator is already registered; use Stake to add stake"
                    .to_string(),
            );
        }
        if let Some(existing_pk) = pool.fingerprint_owner(&fingerprint) {
            if existing_pk != &validator_pubkey {
                return Err(format!(
                    "RegisterValidator: machine fingerprint already registered to {}",
                    existing_pk.to_base58()
                ));
            }
        }

        let mut account = self.b_get_account(&validator_pubkey)?.ok_or_else(|| {
            "RegisterValidator: self-funded validator account not found".to_string()
        })?;
        self.ensure_native_account_direction_not_restricted(
            &validator_pubkey,
            RestrictionTransferDirection::Outgoing,
            amount,
            account.spendable,
            "RegisterValidator",
            "validator",
        )?;
        account
            .stake(amount)
            .map_err(|e| format!("RegisterValidator: self-funded stake failed: {}", e))?;
        self.b_put_account(&validator_pubkey, &account)?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        pool.stake_with_index(validator_pubkey, amount, current_slot, u64::MAX)
            .map_err(|e| format!("RegisterValidator: stake pool error: {}", e))?;
        pool.register_fingerprint(&validator_pubkey, fingerprint)
            .map_err(|e| format!("RegisterValidator: stake pool error: {}", e))?;
        self.b_put_stake_pool(&pool)?;

        Ok(())
    }

    /// System program: ReclassifyValidatorBootstrap (opcode 38).
    ///
    /// Accounts: [validator signer]
    ///
    /// Converts an existing exact 100,000 LICN self-funded validator stake into
    /// bootstrap-recovery accounting. No funds are minted or moved; the
    /// validator voluntarily starts repaying the existing validator stake through
    /// the normal bootstrap debt schedule.
    pub(super) fn system_reclassify_validator_bootstrap(
        &self,
        ix: &Instruction,
    ) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ReclassifyValidatorBootstrap requires [validator] account".to_string());
        }
        if ix.data.len() != 1 {
            return Err("ReclassifyValidatorBootstrap: invalid instruction data".to_string());
        }

        let validator_pubkey = ix.accounts[0];
        let account = self.b_get_account(&validator_pubkey)?.ok_or_else(|| {
            "ReclassifyValidatorBootstrap: validator account not found".to_string()
        })?;
        if account.staked != crate::consensus::BOOTSTRAP_GRANT_AMOUNT {
            return Err(format!(
                "ReclassifyValidatorBootstrap: validator staked balance must be exactly {} spores",
                crate::consensus::BOOTSTRAP_GRANT_AMOUNT
            ));
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_stake_pool()?;
        pool.reclassify_self_funded_as_bootstrap(&validator_pubkey, current_slot)
            .map_err(|e| format!("ReclassifyValidatorBootstrap: stake pool error: {}", e))?;
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

        let evidence: crate::consensus::SlashingEvidence = deserialize_legacy_bincode_strict(
            &ix.data[1..],
            SLASHING_EVIDENCE_CODEC_LIMIT_BYTES,
            "slashing evidence",
        )
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
            if !self.is_speculative() {
                self.state.put_metadata(&offense_key, b"1").map_err(|e| {
                    format!(
                        "SlashValidator: failed to persist idempotency marker: {}",
                        e
                    )
                })?;
            }
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

        if !self.is_speculative() {
            self.state.put_metadata(&offense_key, b"1").map_err(|e| {
                format!(
                    "SlashValidator: failed to persist idempotency marker: {}",
                    e
                )
            })?;
        }

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
        self.b_queue_pending_validator_change(&change)
            .map_err(|e| format!("DeregisterValidator: failed to queue pending change: {}", e))?;

        Ok(())
    }
}
