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

        let new_spores = account
            .spores
            .checked_add(grant_amount)
            .ok_or_else(|| "RegisterValidator: validator total balance overflow".to_string())?;
        let new_staked = account
            .staked
            .checked_add(grant_amount)
            .ok_or_else(|| "RegisterValidator: validator staked balance overflow".to_string())?;
        let classified_total = account
            .spendable
            .checked_add(new_staked)
            .and_then(|total| total.checked_add(account.locked))
            .ok_or_else(|| {
                "RegisterValidator: validator classified balance overflow".to_string()
            })?;
        if new_spores != classified_total {
            return Err(
                "RegisterValidator: validator account invariant would be violated by grant"
                    .to_string(),
            );
        }
        account.spores = new_spores;
        account.staked = new_staked;
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
        if self.staking_v2_active()? {
            self.system_slash_validator_v2(ix)
        } else {
            self.system_slash_validator_legacy(ix)
        }
    }

    /// Exact historical implementation retained for pre-activation replay.
    fn system_slash_validator_legacy(&self, ix: &Instruction) -> Result<(), String> {
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
            if self.staking_v2_active()? {
                pool.checkpoint_staking_v2_validator(
                    &offending_validator,
                    self.staking_v2_execution_slot()?,
                )?;
            }
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

    /// Proof-carrying, chain-bound, owner-proportional slashing used only after
    /// the coordinated Staking V2 boundary.
    fn system_slash_validator_v2(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() != 2 {
            return Err(
                "SlashValidator V2 requires exactly [reporter, offending_validator]".to_string(),
            );
        }
        if ix.data.len() < 2 {
            return Err("SlashValidator V2: missing evidence data".to_string());
        }
        let reporter = ix.accounts[0];
        let offending_validator = ix.accounts[1];
        let evidence: crate::consensus::SlashingEvidence = deserialize_legacy_bincode_strict(
            &ix.data[1..],
            SLASHING_EVIDENCE_CODEC_LIMIT_BYTES,
            "slashing evidence V2",
        )
        .map_err(|error| format!("SlashValidator V2: invalid evidence encoding: {error}"))?;
        if evidence.reporter != reporter {
            return Err("SlashValidator V2: reporter does not match signed account".to_string());
        }
        if evidence.validator != offending_validator {
            return Err("SlashValidator V2: offender does not match evidence".to_string());
        }

        let execution_slot = self.staking_v2_execution_slot()?;
        let offense_slot = evidence.offense_slot();
        if evidence.evidence_slot != offense_slot {
            return Err(format!(
                "SlashValidator V2: evidence slot {} does not match offense slot {}",
                evidence.evidence_slot, offense_slot
            ));
        }
        if offense_slot > execution_slot {
            return Err("SlashValidator V2: offense is in the future".to_string());
        }
        if execution_slot - offense_slot > crate::consensus::SLASHING_V2_EVIDENCE_WINDOW_SLOTS {
            return Err("SlashValidator V2: evidence is outside its window".to_string());
        }
        let chain_id = self
            .transaction_signing_chain_id()?
            .ok_or_else(|| "SlashValidator V2: chain-id metadata is missing".to_string())?;
        if !evidence.verify_with_chain_id(&chain_id) {
            return Err("SlashValidator V2: evidence proof is invalid".to_string());
        }

        let mut pool = self.b_get_stake_pool()?;
        let offense_epoch = crate::consensus::slot_to_epoch(offense_slot);
        let epoch_state = pool
            .staking_v2_state()
            .ok_or_else(|| "SlashValidator V2: epoch state is missing".to_string())?;
        let offense_snapshot = epoch_state
            .slash_exposure_snapshots
            .get(&offense_epoch)
            .ok_or_else(|| "SlashValidator V2: offense exposure snapshot is missing".to_string())?;
        if !offense_snapshot.validators.contains_key(&reporter) {
            return Err(
                "SlashValidator V2: reporter was not an active validator at the offense epoch"
                    .to_string(),
            );
        }
        let offense_epoch_start = offense_snapshot.start_slot;

        let params = self.genesis_consensus_params()?;
        let (kind, slash_percent) = match &evidence.offense {
            crate::consensus::SlashingOffense::DoubleBlockV2 { .. } => (
                "double_block".to_string(),
                params.slashing_percentage_double_sign,
            ),
            crate::consensus::SlashingOffense::DoublePrevoteV2 { vote_1, .. } => (
                if vote_1.round == 0 {
                    "double_prevote_r0".to_string()
                } else {
                    format!("double_prevote_r{}", vote_1.round)
                },
                params.slashing_percentage_double_vote,
            ),
            crate::consensus::SlashingOffense::DoublePrecommitV2 { vote_1, .. } => (
                if vote_1.round == 0 {
                    "double_precommit_r0".to_string()
                } else {
                    format!("double_precommit_r{}", vote_1.round)
                },
                params.slashing_percentage_double_vote,
            ),
            _ => {
                return Err(
                    "SlashValidator V2: legacy or non-equivocation evidence is forbidden"
                        .to_string(),
                )
            }
        };
        if slash_percent == 0 || slash_percent > 100 {
            return Err(format!(
                "SlashValidator V2: invalid genesis slash percentage {slash_percent}"
            ));
        }
        let marker = format!(
            "slash-v2:{}:{}:{}",
            offending_validator.to_base58(),
            offense_slot,
            kind
        )
        .into_bytes();
        if self
            .b_get_contract_storage(&SYSTEM_PROGRAM_ID, &marker)?
            .is_some()
        {
            return Ok(());
        }

        let native = pool.apply_staking_v2_equivocation_slash(
            &offending_validator,
            offense_slot,
            slash_percent,
            execution_slot,
        )?;
        let mut mossstake_pool = self.b_get_mossstake_pool()?;
        let moss = mossstake_pool
            .apply_staking_v2_slash(native.mossstake_requested_loss, offense_epoch_start)?;
        let moss_loss = moss.actual_loss()?;
        if moss_loss > 0 {
            pool.record_staking_v2_external_slash(moss_loss)?;
            let remaining_validators = pool
                .staking_v2_state()
                .ok_or_else(|| "SlashValidator V2: epoch state disappeared".to_string())?
                .validators
                .keys()
                .filter(|validator| **validator != offending_validator)
                .filter(|validator| {
                    pool.get_stake(validator)
                        .is_some_and(|stake| stake.is_active)
                })
                .copied()
                .collect::<Vec<_>>();
            pool.rebalance_mossstake_allocations(
                mossstake_pool.st_licn_token.total_licn_staked,
                &remaining_validators,
                execution_slot,
            )?;
        }

        for loss in &native.owner_losses {
            let mut account = self
                .b_get_account(&loss.owner)?
                .ok_or_else(|| format!("SlashValidator V2: owner {} is missing", loss.owner))?;
            account.staked = account
                .staked
                .checked_sub(loss.active_stake)
                .ok_or_else(|| {
                    format!("SlashValidator V2: {} active stake underflow", loss.owner)
                })?;
            account.locked = account
                .locked
                .checked_sub(loss.cooling_down_stake)
                .ok_or_else(|| {
                    format!(
                        "SlashValidator V2: {} cooling-down stake underflow",
                        loss.owner
                    )
                })?;
            account.spores = account
                .spores
                .checked_sub(loss.total()?)
                .ok_or_else(|| format!("SlashValidator V2: {} balance underflow", loss.owner))?;
            self.b_put_account(&loss.owner, &account)?;
        }

        let total_loss = native
            .native_loss()?
            .checked_add(moss_loss)
            .ok_or_else(|| "SlashValidator V2: total loss overflow".to_string())?;
        if total_loss > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "SlashValidator V2: treasury pubkey is missing".to_string())?;
            let mut treasury = self
                .b_get_account(&treasury_pubkey)?
                .ok_or_else(|| "SlashValidator V2: treasury account is missing".to_string())?;
            treasury
                .add_spendable(total_loss)
                .map_err(|error| format!("SlashValidator V2: treasury credit failed: {error}"))?;
            self.b_put_account(&treasury_pubkey, &treasury)?;
        }
        self.b_put_stake_pool(&pool)?;
        self.b_put_mossstake_pool(&mossstake_pool)?;
        self.b_put_contract_storage(&SYSTEM_PROGRAM_ID, &marker, b"1")?;
        Ok(())
    }

    fn genesis_consensus_params(&self) -> Result<crate::genesis::ConsensusParams, String> {
        let genesis = self
            .state
            .get_block_by_slot(0)?
            .ok_or_else(|| "genesis block is missing".to_string())?;
        let mut embedded = None;
        for transaction in &genesis.transactions {
            for instruction in &transaction.message.instructions {
                if instruction.program_id == SYSTEM_PROGRAM_ID
                    && instruction.data.len() > 1
                    && instruction.data[0] == 40
                {
                    if embedded.is_some() {
                        return Err("genesis contains duplicate config instructions".to_string());
                    }
                    let config: crate::genesis::GenesisConfig =
                        serde_json::from_slice(&instruction.data[1..])
                            .map_err(|error| format!("invalid embedded genesis config: {error}"))?;
                    config.validate()?;
                    let chain_id = self
                        .transaction_signing_chain_id()?
                        .ok_or_else(|| "chain-id metadata is missing".to_string())?;
                    if config.chain_id != chain_id {
                        return Err(format!(
                            "genesis chain id {} does not match active chain id {}",
                            config.chain_id, chain_id
                        ));
                    }
                    embedded = Some(config.consensus);
                }
            }
        }
        embedded.ok_or_else(|| "genesis config instruction is missing".to_string())
    }

    /// System program: SetValidatorCommission (opcode 39).
    ///
    /// Data: `[39 | commission_bps_u64_le]`; accounts: `[validator]`.
    /// Transaction signature verification authenticates the validator. The
    /// committed V2 schedule enforces the cap, step, and two-epoch notice.
    pub(super) fn system_set_validator_commission(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() != 1 {
            return Err("SetValidatorCommission requires exactly [validator]".to_string());
        }
        if ix.data.len() != 9 {
            return Err(
                "SetValidatorCommission data must be [opcode | commission_bps_u64_le]".to_string(),
            );
        }
        if !self.staking_v2_active()? {
            return Err("SetValidatorCommission requires active Staking V2".to_string());
        }

        let validator = ix.accounts[0];
        let commission_bps = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "SetValidatorCommission invalid basis-point encoding".to_string())?,
        );
        let mut pool = self.b_get_stake_pool()?;
        if pool.get_stake(&validator).is_none() {
            return Err(format!(
                "SetValidatorCommission validator {} is not registered",
                validator.to_base58()
            ));
        }
        pool.request_staking_v2_commission_change(&validator, commission_bps)
            .map_err(|err| format!("SetValidatorCommission: {err}"))?;
        self.b_put_stake_pool(&pool)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::serialize_legacy_bincode;
    use crate::consensus::{SlashingEvidence, SlashingOffense, StakePool, MIN_VALIDATOR_STAKE};
    use crate::genesis::GenesisConfig;
    use crate::mossstake::MossStakePool;
    use crate::{Block, Keypair, Message};
    use tempfile::{tempdir, TempDir};

    const TEST_CHAIN_ID: &str = "lichen-slashing-processor-test";

    struct SlashFixture {
        _temp_dir: TempDir,
        processor: TxProcessor,
        state: StateStore,
        reporter: Keypair,
        offender: Keypair,
        delegator: Keypair,
        treasury: Pubkey,
        genesis_hash: Hash,
    }

    fn setup_slash_fixture() -> SlashFixture {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let reporter = Keypair::generate();
        let offender = Keypair::generate();
        let delegator = Keypair::generate();
        let treasury = Pubkey([0x55; 32]);

        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();
        state
            .put_metadata(CHAIN_ID_METADATA_KEY, TEST_CHAIN_ID.as_bytes())
            .unwrap();
        state
            .put_metadata(
                crate::consensus::STAKING_V2_ACTIVATION_SLOT_METADATA_KEY,
                &0u64.to_le_bytes(),
            )
            .unwrap();
        let mut fee_config = FeeConfig::default_from_constants();
        fee_config.base_fee = 0;
        state.set_fee_config_full(&fee_config).unwrap();

        let mut genesis_config = GenesisConfig::default_testnet();
        genesis_config.chain_id = TEST_CHAIN_ID.to_string();
        let mut genesis_data = vec![40];
        genesis_data.extend_from_slice(&serde_json::to_vec(&genesis_config).unwrap());
        let genesis_tx = Transaction::new(Message::new(
            vec![Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![treasury],
                data: genesis_data,
            }],
            Hash::default(),
        ));
        let genesis = Block::genesis(Hash::default(), 0, vec![genesis_tx]);
        let genesis_hash = genesis.hash();
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();

        let mut pool = StakePool::new();
        pool.stake(offender.pubkey(), MIN_VALIDATOR_STAKE, 0)
            .unwrap();
        pool.stake(reporter.pubkey(), MIN_VALIDATOR_STAKE, 0)
            .unwrap();
        pool.delegate(delegator.pubkey(), &offender.pubkey(), MIN_VALIDATOR_STAKE)
            .unwrap();
        pool.initialize_staking_v2(0, &[reporter.pubkey(), offender.pubkey()])
            .unwrap();
        state.put_stake_pool(&pool).unwrap();
        state.put_mossstake_pool(&MossStakePool::new()).unwrap();

        for keypair in [&reporter, &offender, &delegator] {
            let mut account = Account::new(100_000, keypair.pubkey());
            account.stake(MIN_VALIDATOR_STAKE).unwrap();
            state.put_account(&keypair.pubkey(), &account).unwrap();
        }

        SlashFixture {
            _temp_dir: temp_dir,
            processor,
            state,
            reporter,
            offender,
            delegator,
            treasury,
            genesis_hash,
        }
    }

    fn double_block_evidence(fixture: &SlashFixture, timestamp: u64) -> SlashingEvidence {
        let mut block_1 = Block::new_with_timestamp(
            0,
            Hash::hash(b"parent"),
            Hash::hash(b"state-a"),
            fixture.offender.pubkey().0,
            Vec::new(),
            1_700_000_000,
        );
        block_1.sign_with_chain_id(&fixture.offender, TEST_CHAIN_ID);
        let mut block_2 = block_1.clone();
        block_2.header.state_root = Hash::hash(b"state-b");
        block_2.sign_with_chain_id(&fixture.offender, TEST_CHAIN_ID);
        SlashingEvidence::new(
            SlashingOffense::DoubleBlockV2 {
                header_1: block_1.header,
                header_2: block_2.header,
            },
            fixture.offender.pubkey(),
            0,
            fixture.reporter.pubkey(),
            timestamp,
        )
    }

    fn slash_instruction(fixture: &SlashFixture, evidence: &SlashingEvidence) -> Instruction {
        let mut data = vec![27];
        data.extend_from_slice(
            &serialize_legacy_bincode(evidence, "test slashing evidence").unwrap(),
        );
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![fixture.reporter.pubkey(), fixture.offender.pubkey()],
            data,
        }
    }

    fn signed_slash_transaction(
        fixture: &SlashFixture,
        evidence: &SlashingEvidence,
        signer: &Keypair,
    ) -> Transaction {
        let mut transaction = Transaction::new(Message::new(
            vec![slash_instruction(fixture, evidence)],
            fixture.genesis_hash,
        ));
        transaction.signatures.push(
            signer.sign(
                &transaction
                    .message
                    .signing_bytes_for_chain_id(TEST_CHAIN_ID),
            ),
        );
        transaction
    }

    #[test]
    fn staking_v2_slash_transaction_is_reporter_signed_conserving_and_idempotent() {
        let fixture = setup_slash_fixture();
        let evidence = double_block_evidence(&fixture, 1_700_000_001);
        let transaction = signed_slash_transaction(&fixture, &evidence, &fixture.reporter);
        let result = fixture
            .processor
            .process_transaction(&transaction, &fixture.reporter.pubkey());
        assert!(
            result.success,
            "slash transaction failed: {:?}",
            result.error
        );
        assert_eq!(result.fee_paid, 0);

        let expected_owner_loss = MIN_VALIDATOR_STAKE / 2;
        let expected_total_loss = expected_owner_loss * 2;
        let offender_account = fixture
            .state
            .get_account(&fixture.offender.pubkey())
            .unwrap()
            .unwrap();
        let delegator_account = fixture
            .state
            .get_account(&fixture.delegator.pubkey())
            .unwrap()
            .unwrap();
        let treasury_account = fixture
            .state
            .get_account(&fixture.treasury)
            .unwrap()
            .unwrap();
        assert_eq!(
            offender_account.staked,
            MIN_VALIDATOR_STAKE - expected_owner_loss
        );
        assert_eq!(
            delegator_account.staked,
            MIN_VALIDATOR_STAKE - expected_owner_loss
        );
        assert_eq!(treasury_account.spendable, expected_total_loss);
        assert_eq!(treasury_account.spores, expected_total_loss);

        let pool = fixture.state.get_stake_pool().unwrap();
        let offender_stake = pool.get_stake(&fixture.offender.pubkey()).unwrap();
        assert_eq!(
            offender_stake.amount,
            MIN_VALIDATOR_STAKE - expected_owner_loss
        );
        assert_eq!(
            offender_stake.delegated_amount,
            MIN_VALIDATOR_STAKE - expected_owner_loss
        );
        assert!(!offender_stake.is_active);
        assert_eq!(pool.total_slashed(), expected_total_loss);
        let marker = format!(
            "slash-v2:{}:0:double_block",
            fixture.offender.pubkey().to_base58()
        );
        assert_eq!(
            fixture
                .state
                .get_contract_storage(&SYSTEM_PROGRAM_ID, marker.as_bytes())
                .unwrap(),
            Some(b"1".to_vec())
        );

        let pool_hash_after_first = pool.canonical_hash();
        let evidence_again = double_block_evidence(&fixture, 1_700_000_002);
        let transaction_again =
            signed_slash_transaction(&fixture, &evidence_again, &fixture.reporter);
        let repeated = fixture
            .processor
            .process_transaction(&transaction_again, &fixture.reporter.pubkey());
        assert!(
            repeated.success,
            "idempotent retry failed: {:?}",
            repeated.error
        );
        assert_eq!(
            fixture.state.get_stake_pool().unwrap().canonical_hash(),
            pool_hash_after_first
        );
        assert_eq!(
            fixture
                .state
                .get_account(&fixture.treasury)
                .unwrap()
                .unwrap()
                .spendable,
            expected_total_loss
        );
    }

    #[test]
    fn staking_v2_slash_rejects_offender_signature_and_wrong_chain_proof() {
        let fixture = setup_slash_fixture();
        let evidence = double_block_evidence(&fixture, 1_700_000_001);
        let before = fixture.state.get_stake_pool().unwrap().canonical_hash();
        let offender_signed = signed_slash_transaction(&fixture, &evidence, &fixture.offender);
        let result = fixture
            .processor
            .process_transaction(&offender_signed, &fixture.reporter.pubkey());
        assert!(!result.success);
        assert!(result.error.unwrap_or_default().contains("signature"));
        assert_eq!(
            fixture.state.get_stake_pool().unwrap().canonical_hash(),
            before
        );

        let mut wrong_chain = evidence;
        if let SlashingOffense::DoubleBlockV2 { header_1, header_2 } = &mut wrong_chain.offense {
            header_1.signature = Some(fixture.offender.sign(
                &crate::signing::maybe_versioned_signing_bytes(
                    crate::signing::DOMAIN_BLOCK,
                    "another-chain",
                    &header_1.signable_hash().0,
                ),
            ));
            header_2.signature = Some(fixture.offender.sign(
                &crate::signing::maybe_versioned_signing_bytes(
                    crate::signing::DOMAIN_BLOCK,
                    "another-chain",
                    &header_2.signable_hash().0,
                ),
            ));
        }
        let wrong_chain_tx = signed_slash_transaction(&fixture, &wrong_chain, &fixture.reporter);
        let result = fixture
            .processor
            .process_transaction(&wrong_chain_tx, &fixture.reporter.pubkey());
        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("proof is invalid"));
        assert_eq!(
            fixture.state.get_stake_pool().unwrap().canonical_hash(),
            before
        );
    }

    #[test]
    fn staking_v2_slash_rolls_back_every_mutation_on_late_treasury_failure() {
        let fixture = setup_slash_fixture();
        let missing_treasury = Pubkey([0x77; 32]);
        fixture
            .state
            .set_treasury_pubkey(&missing_treasury)
            .unwrap();
        let evidence = double_block_evidence(&fixture, 1_700_000_001);
        let transaction = signed_slash_transaction(&fixture, &evidence, &fixture.reporter);
        let pool_before = fixture.state.get_stake_pool().unwrap().canonical_hash();
        let moss_before = fixture.state.get_mossstake_pool().unwrap().canonical_hash();
        let offender_before = fixture
            .state
            .get_account(&fixture.offender.pubkey())
            .unwrap()
            .unwrap();
        let delegator_before = fixture
            .state
            .get_account(&fixture.delegator.pubkey())
            .unwrap()
            .unwrap();

        let result = fixture
            .processor
            .process_transaction(&transaction, &fixture.reporter.pubkey());
        assert!(!result.success);
        assert!(result
            .error
            .unwrap_or_default()
            .contains("treasury account is missing"));
        assert_eq!(
            fixture.state.get_stake_pool().unwrap().canonical_hash(),
            pool_before
        );
        assert_eq!(
            fixture.state.get_mossstake_pool().unwrap().canonical_hash(),
            moss_before
        );
        let offender_after = fixture
            .state
            .get_account(&fixture.offender.pubkey())
            .unwrap()
            .unwrap();
        let delegator_after = fixture
            .state
            .get_account(&fixture.delegator.pubkey())
            .unwrap()
            .unwrap();
        assert_eq!(offender_after.spores, offender_before.spores);
        assert_eq!(offender_after.staked, offender_before.staked);
        assert_eq!(delegator_after.spores, delegator_before.spores);
        assert_eq!(delegator_after.staked, delegator_before.staked);
        let marker = format!(
            "slash-v2:{}:0:double_block",
            fixture.offender.pubkey().to_base58()
        );
        assert!(fixture
            .state
            .get_contract_storage(&SYSTEM_PROGRAM_ID, marker.as_bytes())
            .unwrap()
            .is_none());
    }
}
