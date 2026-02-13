// MoltChain Consensus Module
// Byzantine Fault Tolerant consensus with Proof of Contribution

use crate::{Hash, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// STAKING - Economic Security
// ============================================================================

/// Minimum stake required to become a validator (100,000 MOLT — $10,000 at $0.10/MOLT)
pub const MIN_VALIDATOR_STAKE: u64 = 100_000 * 1_000_000_000; // 100k MOLT in shells

/// Transaction block reward (0.9 MOLT per block with transactions — $0.09 at $0.10/MOLT)
pub const TRANSACTION_BLOCK_REWARD: u64 = 900_000_000; // 0.9 MOLT

/// Heartbeat block reward (0.135 MOLT per heartbeat — 15% of transaction reward)
pub const HEARTBEAT_BLOCK_REWARD: u64 = 135_000_000; // 0.135 MOLT

/// Legacy constant for backward compatibility (uses transaction reward)
pub const BLOCK_REWARD: u64 = TRANSACTION_BLOCK_REWARD;

/// Target annual reward pool draw rate (5% = 500 basis points — informational only)
/// MOLT is NOT inflationary — rewards are drawn from the 150M validator rewards pool
pub const ANNUAL_REWARD_RATE_BPS: u64 = 500;

/// Slots per year (assuming 400ms per slot = ~78.8M slots/year)
pub const SLOTS_PER_YEAR: u64 = 78_840_000;

// ============================================================================
// PRICE-BASED REWARDS - Dynamic reward adjustment
// ============================================================================

/// Price oracle interface (testnet uses mock, mainnet uses real oracle)
pub trait PriceOracle: Send + Sync {
    fn get_molt_price_usd(&self) -> f64;
}

/// Mock oracle for testnet (returns $0.10 launch price)
pub struct MockOracle;

impl PriceOracle for MockOracle {
    fn get_molt_price_usd(&self) -> f64 {
        0.10 // Launch price: $0.10/MOLT
    }
}

/// Reward configuration with price-based adjustment
#[derive(Debug, Clone)]
pub struct RewardConfig {
    /// Base transaction reward (0.9 MOLT at $0.10 price)
    pub base_transaction_reward: u64,
    /// Base heartbeat reward (0.135 MOLT at $0.10 price)
    pub base_heartbeat_reward: u64,
    /// Reference USD price ($0.10 launch target)
    pub reference_price_usd: f64,
    /// Maximum reward multiplier (10x when price drops to $0.01)
    pub max_adjustment_multiplier: f64,
    /// Minimum reward multiplier (0.1x when price rises to $1.00)
    pub min_adjustment_multiplier: f64,
}

impl RewardConfig {
    pub fn new() -> Self {
        Self {
            base_transaction_reward: TRANSACTION_BLOCK_REWARD,
            base_heartbeat_reward: HEARTBEAT_BLOCK_REWARD,
            reference_price_usd: 0.10,
            max_adjustment_multiplier: 10.0,
            min_adjustment_multiplier: 0.1,
        }
    }

    /// Calculate adjusted reward based on current price
    /// Formula: reward = base_reward * (reference_price / current_price)
    /// Maintains consistent USD value regardless of MOLT price
    /// AUDIT-FIX 3.3: f64 is used because price inputs are inherently floating-point.
    /// This is acceptable for oracle-driven reward adjustment; the base rewards
    /// (HEARTBEAT_BLOCK_REWARD, TRANSACTION_BLOCK_REWARD) used in consensus are u64 constants.
    pub fn get_adjusted_transaction_reward(&self, current_price_usd: f64) -> u64 {
        if current_price_usd <= 0.0 {
            return self.base_transaction_reward;
        }

        let multiplier = (self.reference_price_usd / current_price_usd)
            .max(self.min_adjustment_multiplier)
            .min(self.max_adjustment_multiplier);

        (self.base_transaction_reward as f64 * multiplier) as u64
    }

    /// Calculate adjusted heartbeat reward
    pub fn get_adjusted_heartbeat_reward(&self, current_price_usd: f64) -> u64 {
        if current_price_usd <= 0.0 {
            return self.base_heartbeat_reward;
        }

        let multiplier = (self.reference_price_usd / current_price_usd)
            .max(self.min_adjustment_multiplier)
            .min(self.max_adjustment_multiplier);

        (self.base_heartbeat_reward as f64 * multiplier) as u64
    }

    /// Get reward adjustment info for display
    pub fn get_adjustment_info(&self, current_price_usd: f64) -> RewardAdjustmentInfo {
        let multiplier = if current_price_usd > 0.0 {
            (self.reference_price_usd / current_price_usd)
                .max(self.min_adjustment_multiplier)
                .min(self.max_adjustment_multiplier)
        } else {
            1.0
        };

        RewardAdjustmentInfo {
            current_price_usd,
            reference_price_usd: self.reference_price_usd,
            multiplier,
            adjusted_transaction_reward: self.get_adjusted_transaction_reward(current_price_usd),
            adjusted_heartbeat_reward: self.get_adjusted_heartbeat_reward(current_price_usd),
        }
    }
}

/// Reward adjustment information for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardAdjustmentInfo {
    pub current_price_usd: f64,
    pub reference_price_usd: f64,
    pub multiplier: f64,
    pub adjusted_transaction_reward: u64,
    pub adjusted_heartbeat_reward: u64,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Bootstrap status for validators earning their stake
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BootstrapStatus {
    Bootstrapping, // Still repaying bootstrap debt
    FullyVested,   // Debt fully repaid, can accept delegations
}

/// Maximum stake per validator (1,000,000 MOLT — $100,000 at $0.10/MOLT)
pub const MAX_VALIDATOR_STAKE: u64 = 1_000_000 * 1_000_000_000; // 1M MOLT in shells

/// Unstake cooldown period (7 days in slots at 400ms/slot)
/// H11 fix: was 604,800 (=seconds in 7 days, only 2.8 days at 400ms/slot)
pub const UNSTAKE_COOLDOWN_SLOTS: u64 = 1_512_000; // 7 * 24 * 60 * 60 * 1000 / 400

// ============================================================================
// EPOCH BOUNDARY HANDLING (T4.2)
// ============================================================================

/// Slots per epoch (432,000 ≈ 2 days at 400 ms slots)
pub const SLOTS_PER_EPOCH: u64 = 432_000;

/// Get the epoch number for a given slot
pub fn slot_to_epoch(slot: u64) -> u64 {
    slot / SLOTS_PER_EPOCH
}

/// Get the first slot of an epoch
pub fn epoch_start_slot(epoch: u64) -> u64 {
    epoch * SLOTS_PER_EPOCH
}

/// Check if a slot is the first slot of a new epoch
/// AUDIT-FIX 3.21: Use modulo instead of nightly-only is_multiple_of
pub fn is_epoch_boundary(slot: u64) -> bool {
    slot > 0 && slot % SLOTS_PER_EPOCH == 0
}

/// Summary of an epoch's parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochInfo {
    pub epoch: u64,
    pub start_slot: u64,
    pub end_slot: u64,
    pub total_stake: u64,
    pub validator_count: usize,
}

impl EpochInfo {
    /// Build an EpochInfo for the epoch that contains `slot`
    pub fn for_slot(slot: u64, total_stake: u64, validator_count: usize) -> Self {
        let epoch = slot_to_epoch(slot);
        EpochInfo {
            epoch,
            start_slot: epoch_start_slot(epoch),
            end_slot: epoch_start_slot(epoch + 1) - 1,
            total_stake,
            validator_count,
        }
    }
}

/// Unstake request tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnstakeRequest {
    pub validator: Pubkey,
    /// M5 fix: staker identity to prevent cross-user claims
    pub staker: Pubkey,
    pub amount: u64,
    pub unlock_slot: u64, // When unstake completes
}

/// Stake information for a validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeInfo {
    pub validator: Pubkey,
    pub amount: u64,                  // Total staked amount (including bootstrap)
    pub earned_amount: u64,           // Amount earned through block rewards (real stake)
    pub bootstrap_debt: u64,          // Remaining bootstrap debt to repay
    pub locked_until: u64,            // Slot when stake can be withdrawn
    pub is_active: bool,              // Whether stake is currently active
    pub delegated_amount: u64,        // Amount delegated by others
    pub rewards_earned: u64,          // Total rewards earned (unclaimed)
    pub last_reward_slot: u64,        // Last slot rewards were calculated
    pub status: BootstrapStatus,      // Vesting status
    pub blocks_produced: u64,         // Total blocks produced (for achievements)
    pub graduation_slot: Option<u64>, // Slot when fully vested (if graduated)
    #[serde(default)]
    pub total_claimed: u64, // Total lifetime rewards claimed (liquid + debt)
    #[serde(default)]
    pub total_debt_repaid: u64, // Total lifetime debt repaid
}

impl StakeInfo {
    /// Create new validator stake with bootstrap (Contributory Stake system)
    pub fn new(validator: Pubkey, amount: u64, current_slot: u64) -> Self {
        // Validators start with bootstrap stake and must earn it
        let bootstrap_debt = if amount == MIN_VALIDATOR_STAKE {
            amount // Bootstrap: granted stake, must be earned
        } else {
            0 // Already has stake (existing validator)
        };

        Self {
            validator,
            amount,
            earned_amount: 0, // Starts at 0, increases as debt repaid
            bootstrap_debt,   // Starts at 10k, decreases to 0
            locked_until: current_slot + 1000,
            is_active: amount >= MIN_VALIDATOR_STAKE,
            delegated_amount: 0,
            rewards_earned: 0,
            last_reward_slot: current_slot,
            status: if bootstrap_debt > 0 {
                BootstrapStatus::Bootstrapping
            } else {
                BootstrapStatus::FullyVested
            },
            blocks_produced: 0,
            graduation_slot: None,
            total_claimed: 0,
            total_debt_repaid: 0,
        }
    }

    /// Get total stake including delegations
    /// AUDIT-FIX 1.2c: saturating_add to prevent overflow
    pub fn total_stake(&self) -> u64 {
        self.amount.saturating_add(self.delegated_amount)
    }

    /// Check if stake meets minimum requirement
    pub fn meets_minimum(&self) -> bool {
        self.total_stake() >= MIN_VALIDATOR_STAKE
    }

    /// Slash stake by amount (returns amount actually slashed)
    /// AUDIT-FIX 3.4: Design is additive slashing on principal with proportional
    /// vesting adjustment. amount is absolute shells to slash (not a percentage).
    /// Vesting fields (earned_amount, bootstrap_debt) are scaled proportionally
    /// to maintain consistent vesting ratios after the principal is reduced.
    pub fn slash(&mut self, amount: u64) -> u64 {
        let slashed = amount.min(self.amount);
        if slashed == 0 || self.amount == 0 {
            return 0;
        }
        // Proportionally reduce vesting state to keep ratios consistent
        let ratio_num = self.amount.saturating_sub(slashed);
        let ratio_den = self.amount;
        self.earned_amount =
            (self.earned_amount as u128 * ratio_num as u128 / ratio_den as u128) as u64;
        self.bootstrap_debt =
            (self.bootstrap_debt as u128 * ratio_num as u128 / ratio_den as u128) as u64;
        self.amount = ratio_num;
        self.is_active = self.meets_minimum();
        slashed
    }

    /// Add block reward to accumulated rewards
    /// AUDIT-FIX 1.2a: saturating_add to prevent overflow
    pub fn add_reward(&mut self, reward: u64, slot: u64) {
        self.rewards_earned = self.rewards_earned.saturating_add(reward);
        self.last_reward_slot = slot;
    }

    /// Claim accumulated rewards with Contributory Stake split
    /// Returns (liquid_amount, debt_payment)
    pub fn claim_rewards(&mut self) -> (u64, u64) {
        let total_reward = self.rewards_earned;
        self.rewards_earned = 0;

        if self.bootstrap_debt > 0 {
            // Bootstrapping: 50/50 split (debt repayment vs liquid)
            let debt_payment = total_reward / 2;

            // Apply debt payment (capped at remaining debt)
            let paid = debt_payment.min(self.bootstrap_debt);
            self.bootstrap_debt -= paid;
            // AUDIT-FIX 1.2b: saturating_add to prevent overflow
            self.earned_amount = self.earned_amount.saturating_add(paid);
            self.total_debt_repaid = self.total_debt_repaid.saturating_add(paid);

            // Liquid = everything not going to debt (includes excess if debt < half reward)
            let liquid = total_reward - paid;

            // Track total claimed (liquid + debt payment = full reward)
            self.total_claimed = self.total_claimed.saturating_add(total_reward);

            // Check for graduation
            if self.bootstrap_debt == 0 {
                self.status = BootstrapStatus::FullyVested;
                self.graduation_slot = Some(self.last_reward_slot);
            }

            (liquid, paid) // (spendable, locked_for_debt)
        } else {
            // Fully vested: 100% liquid
            self.total_claimed = self.total_claimed.saturating_add(total_reward);
            (total_reward, 0)
        }
    }

    /// Check if validator is fully vested
    pub fn is_fully_vested(&self) -> bool {
        self.status == BootstrapStatus::FullyVested
    }

    /// Get vesting progress (0-100%)
    pub fn vesting_progress(&self) -> u64 {
        if self.earned_amount == 0 && self.bootstrap_debt == 0 {
            return 100; // No bootstrap, fully vested
        }
        let total_bootstrap = self.earned_amount + self.bootstrap_debt;
        if total_bootstrap == 0 {
            return 100;
        }
        (self.earned_amount * 100) / total_bootstrap
    }

    /// Calculate staking APY based on current stake and total staked
    pub fn calculate_apy(&self, total_staked: u64) -> f64 {
        if total_staked == 0 {
            return 0.0;
        }
        // APY = (annual_inflation / total_staked) * 100
        // Higher stake concentration = lower individual APY
        // AUDIT-FIX 3.3: APY is display-only (not consensus-critical), f64 is acceptable
        let annual_rewards = (BLOCK_REWARD * SLOTS_PER_YEAR) as f64;
        (annual_rewards / total_staked as f64) * 100.0
    }

    /// Add additional stake (after graduation, up to 100k max)
    pub fn add_stake(&mut self, additional: u64) -> Result<(), String> {
        if !self.is_fully_vested() {
            return Err("Must be fully vested to add additional stake".to_string());
        }

        if self.amount + additional > MAX_VALIDATOR_STAKE {
            return Err(format!(
                "Cannot exceed maximum stake of {} MOLT",
                MAX_VALIDATOR_STAKE / 1_000_000_000
            ));
        }

        self.amount += additional;
        Ok(())
    }

    /// Request to unstake (stop validating and withdraw stake)
    pub fn request_unstake(
        &self,
        amount: u64,
        current_slot: u64,
        staker: Pubkey,
    ) -> Result<UnstakeRequest, String> {
        if !self.is_fully_vested() {
            return Err("Must be fully vested to unstake".to_string());
        }

        if amount == 0 || amount > self.amount {
            return Err("Invalid unstake amount".to_string());
        }

        Ok(UnstakeRequest {
            validator: self.validator,
            staker, // M5 fix: track staker identity
            amount,
            unlock_slot: current_slot + UNSTAKE_COOLDOWN_SLOTS,
        })
    }
}

/// Manages all validator stakes in the network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakePool {
    stakes: HashMap<Pubkey, StakeInfo>,
    total_staked: u64,
    total_slashed: u64,
    /// AUDIT-FIX 1.4: Keyed by (validator, staker) to support concurrent unstakes
    unstake_requests: HashMap<(Pubkey, Pubkey), UnstakeRequest>,
    /// Per-validator map of delegator -> amount
    #[serde(default)]
    delegations: HashMap<Pubkey, HashMap<Pubkey, u64>>,
}

impl Default for StakePool {
    fn default() -> Self {
        Self::new()
    }
}

impl StakePool {
    pub fn new() -> Self {
        Self {
            stakes: HashMap::new(),
            total_staked: 0,
            total_slashed: 0,
            unstake_requests: HashMap::new(),
            delegations: HashMap::new(),
        }
    }

    /// Register stake for a validator
    pub fn stake(
        &mut self,
        validator: Pubkey,
        amount: u64,
        current_slot: u64,
    ) -> Result<(), String> {
        if amount < MIN_VALIDATOR_STAKE {
            return Err(format!(
                "Stake {} is below minimum {}",
                amount, MIN_VALIDATOR_STAKE
            ));
        }

        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            stake_info.amount += amount;
            stake_info.is_active = stake_info.meets_minimum();
        } else {
            let stake_info = StakeInfo::new(validator, amount, current_slot);
            self.stakes.insert(validator, stake_info);
        }

        self.total_staked += amount;
        Ok(())
    }

    /// Upsert stake for a validator (used for network-synced stake snapshots)
    pub fn upsert_stake(&mut self, validator: Pubkey, amount: u64, current_slot: u64) {
        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            let old_amount = stake_info.amount;
            if amount > old_amount {
                self.total_staked = self.total_staked.saturating_add(amount - old_amount);
            } else {
                self.total_staked = self.total_staked.saturating_sub(old_amount - amount);
            }
            stake_info.amount = amount;
            stake_info.is_active = stake_info.meets_minimum();
        } else {
            let stake_info = StakeInfo::new(validator, amount, current_slot);
            self.total_staked = self.total_staked.saturating_add(amount);
            self.stakes.insert(validator, stake_info);
        }
    }

    /// Slash validator stake (returns amount slashed)
    pub fn slash_validator(&mut self, validator: &Pubkey, amount: u64) -> u64 {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            let slashed = stake_info.slash(amount);
            self.total_staked = self.total_staked.saturating_sub(slashed);
            self.total_slashed += slashed;
            slashed
        } else {
            0
        }
    }

    /// Get stake info for a validator
    pub fn get_stake(&self, validator: &Pubkey) -> Option<&StakeInfo> {
        self.stakes.get(validator)
    }

    /// Get deterministic stake entries (sorted by pubkey)
    pub fn stake_entries(&self) -> Vec<StakeInfo> {
        let mut entries: Vec<StakeInfo> = self.stakes.values().cloned().collect();
        entries.sort_by_key(|info| info.validator.0);
        entries
    }

    /// Get total stake in the network (already excludes pending unstakes)
    pub fn total_stake(&self) -> u64 {
        self.total_staked
    }

    /// Get total *active* stake eligible for rewards and leader selection (T6.3).
    ///
    /// This equals `total_stake()` because `request_unstake()` immediately subtracts
    /// unstaked amounts from both `total_staked` and individual `StakeInfo::amount`.
    /// Pending unstakes do NOT dilute active stakers.
    pub fn active_stake(&self) -> u64 {
        self.total_staked
    }

    /// Get the total amount currently locked in pending unstake requests (T6.3).
    /// These tokens are no longer counted in active stake or reward calculations.
    pub fn pending_unstake_total(&self) -> u64 {
        self.unstake_requests.values().map(|r| r.amount).sum()
    }

    /// Get total slashed amount (burned)
    pub fn total_slashed(&self) -> u64 {
        self.total_slashed
    }

    /// Get all active validators (meeting minimum stake)
    pub fn active_validators(&self) -> Vec<Pubkey> {
        self.stakes
            .iter()
            .filter(|(_, info)| info.is_active && info.meets_minimum())
            .map(|(pubkey, _)| *pubkey)
            .collect()
    }

    /// Calculate stake-weighted voting power (normalized 0-100)
    pub fn voting_power(&self, validator: &Pubkey) -> u64 {
        // T6.3: Use active_stake() to ensure pending unstakes don't dilute voting power
        let active = self.active_stake();
        if active == 0 {
            return 0;
        }
        if let Some(stake_info) = self.stakes.get(validator) {
            if stake_info.is_active {
                // Return proportional voting power (0-10000 for precision, divide by 100 for 0-100)
                ((stake_info.total_stake() as u128 * 10000) / active as u128) as u64
            } else {
                0
            }
        } else {
            0
        }
    }

    /// Distribute block reward to validator
    pub fn distribute_block_reward(
        &mut self,
        validator: &Pubkey,
        slot: u64,
        is_heartbeat: bool,
    ) -> u64 {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            if stake_info.is_active {
                let reward = if is_heartbeat {
                    HEARTBEAT_BLOCK_REWARD
                } else {
                    TRANSACTION_BLOCK_REWARD
                };
                stake_info.add_reward(reward, slot);
                return reward;
            }
        }
        0
    }

    /// Distribute transaction fees to validator
    pub fn distribute_fees(&mut self, validator: &Pubkey, fees: u64, slot: u64) {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            if stake_info.is_active {
                stake_info.add_reward(fees, slot);
            }
        }
    }

    /// Claim rewards for validator (returns (liquid, debt_payment))
    pub fn claim_rewards(&mut self, validator: &Pubkey) -> (u64, u64) {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.claim_rewards()
        } else {
            (0, 0)
        }
    }

    /// Record block production (for achievements)
    pub fn record_block_produced(&mut self, validator: &Pubkey) {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.blocks_produced += 1;
        }
    }

    /// Delegate stake to a fully vested validator
    pub fn delegate(
        &mut self,
        delegator: Pubkey,
        validator: &Pubkey,
        amount: u64,
    ) -> Result<(), String> {
        // Check validator is fully vested
        let stake_info = self
            .stakes
            .get(validator)
            .ok_or_else(|| "Validator not found".to_string())?;

        if !stake_info.is_fully_vested() {
            return Err("Validator still bootstrapping, cannot accept delegations".to_string());
        }

        // Update validator's delegated amount
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.delegated_amount += amount;
        }

        // Delegations contribute to total active stake (used as denominator
        // in reward distribution and voting power calculations).
        self.total_staked += amount;

        // Track individual delegation
        let validator_delegations = self.delegations.entry(*validator).or_default();
        let entry = validator_delegations.entry(delegator).or_insert(0);
        *entry += amount;

        Ok(())
    }

    /// Undelegate stake from validator
    pub fn undelegate(
        &mut self,
        delegator: Pubkey,
        validator: &Pubkey,
        amount: u64,
    ) -> Result<(), String> {
        // Check delegator has sufficient delegation
        let delegated = self
            .delegations
            .get(validator)
            .and_then(|m| m.get(&delegator))
            .copied()
            .unwrap_or(0);

        if delegated < amount {
            return Err(format!(
                "Insufficient delegation: have {}, requested {}",
                delegated, amount
            ));
        }

        if let Some(stake_info) = self.stakes.get_mut(validator) {
            if stake_info.delegated_amount < amount {
                return Err("Insufficient delegated amount".to_string());
            }
            stake_info.delegated_amount -= amount;
        } else {
            return Err("Validator not found".to_string());
        }

        // Mirror delegate(): remove delegated amount from total active stake
        self.total_staked = self.total_staked.saturating_sub(amount);

        // Update individual delegation tracking
        if let Some(validator_delegations) = self.delegations.get_mut(validator) {
            if let Some(entry) = validator_delegations.get_mut(&delegator) {
                *entry -= amount;
                if *entry == 0 {
                    validator_delegations.remove(&delegator);
                }
            }
            if validator_delegations.is_empty() {
                self.delegations.remove(validator);
            }
        }

        Ok(())
    }

    /// Add additional stake to validator (after graduation, up to 100k max)
    pub fn add_validator_stake(
        &mut self,
        validator: &Pubkey,
        additional: u64,
    ) -> Result<(), String> {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.add_stake(additional)?;
            self.total_staked += additional;
            Ok(())
        } else {
            Err("Validator not found".to_string())
        }
    }

    /// Request unstake (stop validating, start cooldown)
    pub fn request_unstake(
        &mut self,
        validator: &Pubkey,
        amount: u64,
        current_slot: u64,
        staker: Pubkey,
    ) -> Result<UnstakeRequest, String> {
        let stake_info = self
            .stakes
            .get(validator)
            .ok_or_else(|| "Validator not found".to_string())?;

        // Check if already unstaking
        // AUDIT-FIX 1.4: Key by (validator, staker) so different stakers can
        // unstake from the same validator concurrently.
        let unstake_key = (*validator, staker);
        if self.unstake_requests.contains_key(&unstake_key) {
            return Err("Unstake already in progress".to_string());
        }

        // Create unstake request
        let request = stake_info.request_unstake(amount, current_slot, staker)?;
        self.unstake_requests.insert(unstake_key, request.clone());

        // Deactivate validator immediately (can't produce blocks during cooldown)
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.amount = stake_info.amount.saturating_sub(request.amount);
            stake_info.is_active = stake_info.meets_minimum();
        }

        self.total_staked = self.total_staked.saturating_sub(request.amount);

        Ok(request)
    }

    /// Claim unstake after cooldown completes
    pub fn claim_unstake(
        &mut self,
        validator: &Pubkey,
        current_slot: u64,
        staker: &Pubkey,
    ) -> Result<u64, String> {
        // AUDIT-FIX 1.4: Look up by (validator, staker) composite key
        let unstake_key = (*validator, *staker);
        let request = self
            .unstake_requests
            .get(&unstake_key)
            .ok_or_else(|| "No unstake request found".to_string())?;

        // AUDIT-FIX 1.4: staker identity is implicit in the composite key —
        // no need for separate staker check.

        // Check if cooldown completed
        if current_slot < request.unlock_slot {
            let remaining = request.unlock_slot - current_slot;
            return Err(format!(
                "Cooldown not complete. {} slots remaining (~{} hours)",
                remaining,
                remaining * 400 / 1000 / 3600 // Convert to hours
            ));
        }

        let amount = request.amount;

        // AUDIT-FIX 1.4: remove by composite key
        self.unstake_requests.remove(&unstake_key);

        Ok(amount)
    }

    /// Get unstake request for validator+staker pair
    pub fn get_unstake_request(&self, validator: &Pubkey) -> Option<&UnstakeRequest> {
        // Backward-compat: search for any request matching this validator
        self.unstake_requests.iter()
            .find(|((v, _), _)| v == validator)
            .map(|(_, req)| req)
    }

    /// Get total unclaimed rewards in pool
    pub fn total_unclaimed_rewards(&self) -> u64 {
        self.stakes.values().map(|s| s.rewards_earned).sum()
    }

    /// Get network staking statistics
    pub fn get_stats(&self) -> StakingStats {
        let active_validators = self.active_validators().len() as u64;
        let unclaimed_rewards = self.total_unclaimed_rewards();
        let avg_stake = if active_validators > 0 {
            self.total_staked / active_validators
        } else {
            0
        };

        StakingStats {
            total_staked: self.total_staked,
            total_slashed: self.total_slashed,
            total_unclaimed_rewards: unclaimed_rewards,
            active_validators,
            average_stake: avg_stake,
        }
    }

    /// Get all delegations for a specific validator
    pub fn get_delegations(&self, validator: &Pubkey) -> Vec<(Pubkey, u64)> {
        self.delegations
            .get(validator)
            .map(|m| m.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default()
    }

    /// Get all delegations made by a specific delegator (across all validators)
    pub fn get_delegator_stakes(&self, delegator: &Pubkey) -> Vec<(Pubkey, u64)> {
        let mut result = Vec::new();
        for (validator, delegations) in &self.delegations {
            if let Some(&amount) = delegations.get(delegator) {
                result.push((*validator, amount));
            }
        }
        result
    }

    /// T4.5: Distribute epoch rewards to all validators proportional to their stake.
    /// Returns a list of (validator_pubkey, reward_amount). The caller is responsible
    /// for crediting accounts during epoch transitions.
    pub fn distribute_epoch_rewards(&self, epoch_reward_pool: u64) -> Vec<(Pubkey, u64)> {
        // T6.3: Use active stake only — pending unstakes already excluded from total_staked
        let total_stake = self.active_stake();
        if total_stake == 0 || epoch_reward_pool == 0 {
            return vec![];
        }

        let mut distributions = Vec::new();
        for stake_info in self.stakes.values() {
            if !stake_info.is_active {
                continue;
            }
            let validator_share = (epoch_reward_pool as u128)
                .checked_mul(stake_info.total_stake() as u128)
                .unwrap_or(0)
                / total_stake as u128;

            if validator_share > 0 {
                distributions.push((stake_info.validator, validator_share as u64));
            }
        }

        // Sort deterministically by pubkey
        distributions.sort_by_key(|(pk, _)| pk.0);
        distributions
    }

    /// Distribute delegation rewards proportionally to delegators
    /// Validator keeps `commission_bps` basis points (e.g. 1000 = 10%)
    /// Returns Vec<(delegator, reward_amount)>
    pub fn distribute_delegation_rewards(
        &mut self,
        validator: &Pubkey,
        total_reward: u64,
        commission_bps: u64,
    ) -> Vec<(Pubkey, u64)> {
        let stake_info = match self.stakes.get(validator) {
            Some(s) => s,
            None => return vec![],
        };

        let delegated = stake_info.delegated_amount;
        if delegated == 0 || total_reward == 0 {
            return vec![];
        }

        // Split reward between validator's own stake and delegated stake
        let total_stake = stake_info.total_stake();
        if total_stake == 0 {
            return vec![];
        }
        let delegation_share =
            (total_reward as u128 * delegated as u128 / total_stake as u128) as u64;

        // Validator takes commission from delegation share
        let commission = (delegation_share as u128 * commission_bps as u128 / 10_000) as u64;
        let distributable = delegation_share.saturating_sub(commission);

        // Distribute proportionally to each delegator
        let delegator_map = match self.delegations.get(validator) {
            Some(m) => m.clone(),
            None => return vec![],
        };

        let mut rewards = Vec::new();
        // H9 fix: Credit commission to the validator (was silently burned before)
        if commission > 0 {
            rewards.push((*validator, commission));
        }
        for (delegator, amount) in &delegator_map {
            if delegated > 0 {
                let share = (distributable as u128 * *amount as u128 / delegated as u128) as u64;
                if share > 0 {
                    rewards.push((*delegator, share));
                }
            }
        }

        rewards
    }
}

/// Staking statistics for the network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingStats {
    pub total_staked: u64,
    pub total_slashed: u64,
    pub total_unclaimed_rewards: u64,
    pub active_validators: u64,
    pub average_stake: u64,
}

// Helper functions for [u8; 64] signature serialization
fn serialize_signature<S>(sig: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    hex::encode(sig).serialize(serializer)
}

fn deserialize_signature<'de, D>(deserializer: D) -> Result<[u8; 64], D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let hex_str: String = String::deserialize(deserializer)?;
    let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
    if bytes.len() != 64 {
        return Err(serde::de::Error::custom("Invalid signature length"));
    }
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bytes);
    Ok(sig)
}

/// Consensus vote for a block
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vote {
    /// Slot number being voted on
    pub slot: u64,
    /// Hash of the block being voted for
    pub block_hash: Hash,
    /// Validator who cast this vote
    pub validator: Pubkey,
    /// Ed25519 signature over (slot, block_hash)
    #[serde(
        serialize_with = "serialize_signature",
        deserialize_with = "deserialize_signature"
    )]
    pub signature: [u8; 64],
    /// Timestamp when vote was created
    pub timestamp: u64,
}

impl Vote {
    /// Create a new vote
    pub fn new(slot: u64, block_hash: Hash, validator: Pubkey, signature: [u8; 64]) -> Self {
        Vote {
            slot,
            block_hash,
            validator,
            signature,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Verify vote signature
    pub fn verify(&self) -> bool {
        // Construct message: slot || block_hash
        let mut message = Vec::new();
        message.extend_from_slice(&self.slot.to_le_bytes());
        message.extend_from_slice(&self.block_hash.0);

        crate::Keypair::verify(&self.validator, &message, &self.signature)
    }
}

/// Validator information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    /// Validator's public key
    pub pubkey: Pubkey,
    /// Reputation score (0-1000)
    pub reputation: u64,
    /// Total blocks proposed
    pub blocks_proposed: u64,
    /// Total votes cast
    pub votes_cast: u64,
    /// Correct votes (voted for finalized blocks)
    pub correct_votes: u64,
    /// Stake amount in shells (for future use)
    pub stake: u64,
    /// When validator joined
    pub joined_slot: u64,
    /// Last slot validator was active
    pub last_active_slot: u64,
}

impl ValidatorInfo {
    /// Create new validator
    pub fn new(pubkey: Pubkey, joined_slot: u64) -> Self {
        ValidatorInfo {
            pubkey,
            reputation: 500, // Start at mid-reputation
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 0,
            joined_slot,
            last_active_slot: joined_slot,
        }
    }

    /// Update reputation based on performance
    pub fn update_reputation(&mut self, correct: bool) {
        if correct {
            // Increase reputation (max 1000)
            self.reputation = (self.reputation + 10).min(1000);
        } else {
            // Decrease reputation (min 0)
            self.reputation = self.reputation.saturating_sub(50);
        }
    }

    /// Get voting weight (reputation-based)
    pub fn voting_weight(&self) -> u64 {
        self.reputation
    }
}

/// Validator set management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    validators: Vec<ValidatorInfo>,
}

impl Default for ValidatorSet {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidatorSet {
    /// Create empty validator set
    pub fn new() -> Self {
        ValidatorSet {
            validators: Vec::new(),
        }
    }

    /// Add validator to set
    pub fn add_validator(&mut self, info: ValidatorInfo) {
        if !self.validators.iter().any(|v| v.pubkey == info.pubkey) {
            self.validators.push(info);
        }
    }

    /// Remove validator from set
    pub fn remove_validator(&mut self, pubkey: &Pubkey) {
        self.validators.retain(|v| v.pubkey != *pubkey);
    }

    /// Get validator by pubkey
    pub fn get_validator(&self, pubkey: &Pubkey) -> Option<&ValidatorInfo> {
        self.validators.iter().find(|v| v.pubkey == *pubkey)
    }

    /// Get mutable validator by pubkey
    pub fn get_validator_mut(&mut self, pubkey: &Pubkey) -> Option<&mut ValidatorInfo> {
        self.validators.iter_mut().find(|v| v.pubkey == *pubkey)
    }

    /// Get all validators
    pub fn validators(&self) -> &[ValidatorInfo] {
        &self.validators
    }

    /// Get validators in deterministic order
    pub fn sorted_validators(&self) -> Vec<ValidatorInfo> {
        let mut sorted = self.validators.clone();
        sorted.sort_by_key(|v| v.pubkey.0);
        sorted
    }

    /// Get all validators mutably
    pub fn validators_mut(&mut self) -> &mut [ValidatorInfo] {
        &mut self.validators
    }

    /// Get total voting weight
    pub fn total_voting_weight(&self) -> u64 {
        self.validators.iter().map(|v| v.voting_weight()).sum()
    }

    /// Select leader for given slot using Proof of Contribution
    /// Uses stake-weighted selection when a StakePool is provided,
    /// falls back to deterministic round-robin otherwise.
    pub fn select_leader(&self, slot: u64) -> Option<Pubkey> {
        self.select_leader_round_robin(slot)
    }

    /// Deterministic round-robin leader selection (fallback when no stake pool)
    fn select_leader_round_robin(&self, slot: u64) -> Option<Pubkey> {
        if self.validators.is_empty() {
            return None;
        }

        let mut sorted_validators = self.validators.clone();
        sorted_validators.sort_by_key(|v| v.pubkey.0);

        let index = (slot as usize) % sorted_validators.len();
        Some(sorted_validators[index].pubkey)
    }

    /// Check if validator is leader for slot (weighted when stake pool provided)
    pub fn is_leader(&self, slot: u64, pubkey: &Pubkey) -> bool {
        self.select_leader(slot)
            .map(|leader| leader == *pubkey)
            .unwrap_or(false)
    }

    /// Check if validator is leader for slot using weighted selection
    pub fn is_leader_weighted(&self, slot: u64, pubkey: &Pubkey, stake_pool: &StakePool) -> bool {
        self.select_leader_weighted(slot, stake_pool)
            .map(|leader| leader == *pubkey)
            .unwrap_or(false)
    }

    /// Select leader using stake-weighted contribution
    pub fn select_leader_weighted(&self, slot: u64, stake_pool: &StakePool) -> Option<Pubkey> {
        if self.validators.is_empty() {
            return None;
        }

        let sorted_validators = self.sorted_validators();

        let weights: Vec<u64> = sorted_validators
            .iter()
            .map(|v| {
                let stake = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake().min(MAX_VALIDATOR_STAKE))
                    .unwrap_or_else(|| v.stake.min(MAX_VALIDATOR_STAKE));

                let base = integer_sqrt(stake.max(1));
                // H10 fix: multiply before divide to preserve granularity
                let reputation = v.reputation.max(100) as u128;
                ((base as u128).saturating_mul(reputation) / 100u128 + 1) as u64
                // +1 avoids zero weight
            })
            .collect();

        let total_weight: u64 = weights.iter().sum();
        if total_weight == 0 {
            return self.select_leader(slot);
        }

        let hash = Hash::hash(&slot.to_le_bytes());
        let mut seed_bytes = [0u8; 8];
        seed_bytes.copy_from_slice(&hash.0[..8]);
        let mut target = u64::from_le_bytes(seed_bytes) % total_weight;

        for (index, weight) in weights.iter().enumerate() {
            if *weight == 0 {
                continue;
            }
            if target < *weight {
                return Some(sorted_validators[index].pubkey);
            }
            target -= *weight;
        }

        Some(sorted_validators[0].pubkey)
    }
}

// AUDIT-FIX 3.3: Use Newton's method with pure integer arithmetic
// instead of f64 intermediate for deterministic consensus results.
fn integer_sqrt(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

/// Calculate governance voting power per whitepaper:
///   voting_power = sqrt(tokens_held) × reputation_multiplier
///   reputation_multiplier = 1.0 + (reputation_score / 1000)
///   Max multiplier = 3.0 (for reputation >= 2000)
pub fn governance_voting_power(tokens_held: u64, reputation: u64) -> u64 {
    let base = integer_sqrt(tokens_held);
    // reputation_multiplier = 1.0 + (reputation / 1000), max 3.0
    // We use fixed-point: multiply by 1000 to avoid floats
    let multiplier_x1000 = 1000u64 + reputation.min(2000);
    // Cap at 3000 (3.0x)
    let capped = multiplier_x1000.min(3000);
    (base as u128 * capped as u128 / 1000) as u64
}

/// Vote aggregator for BFT consensus
#[derive(Debug, Clone)]
pub struct VoteAggregator {
    /// Votes collected per (slot, block_hash)
    votes: std::collections::HashMap<(u64, Hash), Vec<Vote>>,
}

impl Default for VoteAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl VoteAggregator {
    /// Create new vote aggregator
    pub fn new() -> Self {
        VoteAggregator {
            votes: std::collections::HashMap::new(),
        }
    }

    /// Add vote to aggregator (validates signature and membership)
    pub fn add_vote(&mut self, vote: Vote) -> bool {
        // Verify signature
        if !vote.verify() {
            return false;
        }

        // H8 fix: Prevent equivocation — reject second vote from same validator
        // at the same slot, regardless of block hash.
        for ((slot, _hash), votes) in &self.votes {
            if *slot == vote.slot && votes.iter().any(|v| v.validator == vote.validator) {
                return false; // equivocation attempt
            }
        }

        let key = (vote.slot, vote.block_hash);
        let votes = self.votes.entry(key).or_default();
        votes.push(vote);
        true
    }

    /// Add vote with validator set membership check (T4.3).
    /// Only accepts votes from validators in the current set.
    pub fn add_vote_validated(&mut self, vote: Vote, validator_set: &ValidatorSet) -> bool {
        // T4.3: Check that voter is in the current validator set
        if validator_set.get_validator(&vote.validator).is_none() {
            return false;
        }
        self.add_vote(vote)
    }

    /// Get votes for specific block
    pub fn get_votes(&self, slot: u64, block_hash: &Hash) -> Option<&Vec<Vote>> {
        self.votes.get(&(slot, *block_hash))
    }

    /// Check if block has reached BFT threshold (66% of voting weight)
    /// Now uses stake-weighted voting power instead of reputation
    pub fn has_supermajority(
        &self,
        slot: u64,
        block_hash: &Hash,
        validator_set: &ValidatorSet,
        stake_pool: &StakePool,
    ) -> bool {
        let votes = match self.get_votes(slot, block_hash) {
            Some(v) => v,
            None => return false,
        };

        let total_stake = stake_pool.total_stake();
        if total_stake == 0 {
            // Fallback to reputation-based if no stake
            let vote_weight: u64 = votes
                .iter()
                .filter_map(|vote| validator_set.get_validator(&vote.validator))
                .map(|v| v.voting_weight())
                .sum();

            let total_weight = validator_set.total_voting_weight();
            // AUDIT-FIX 1.2d: u128 cast to prevent overflow on multiplication
            return (vote_weight as u128) * 3 >= (total_weight as u128) * 2;
        }

        // Calculate stake-weighted voting power
        let vote_stake: u64 = votes
            .iter()
            .filter_map(|vote| stake_pool.get_stake(&vote.validator))
            .map(|stake_info| stake_info.total_stake())
            .sum();

        // Need 66% (2/3) of stake for supermajority
        // AUDIT-FIX 1.2d: u128 cast to prevent overflow on multiplication
        (vote_stake as u128) * 3 >= (total_stake as u128) * 2
    }

    /// Get vote count for block
    pub fn vote_count(&self, slot: u64, block_hash: &Hash) -> usize {
        self.get_votes(slot, block_hash)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Clear old votes (older than given slot)
    pub fn prune_old_votes(&mut self, current_slot: u64, keep_slots: u64) {
        let cutoff_slot = current_slot.saturating_sub(keep_slots);
        self.votes.retain(|(slot, _), _| *slot >= cutoff_slot);
    }
}

/// T4.1: Fork choice rule — heaviest observed chain
/// Tracks competing chain heads by (slot, block_hash, cumulative_stake_weight)
/// and selects the canonical head using highest-slot-first, then most stake,
/// then deterministic hash tiebreak.
// T7.3: Fork choice with re-execution — implemented Session 4
#[derive(Debug, Clone)]
pub struct ForkChoice {
    /// Known chain heads: (slot, block_hash, cumulative_stake_weight)
    heads: Vec<(u64, Hash, u64)>,
}

impl Default for ForkChoice {
    fn default() -> Self {
        Self::new()
    }
}

impl ForkChoice {
    /// Create new fork choice tracker
    pub fn new() -> Self {
        ForkChoice { heads: Vec::new() }
    }

    /// Record a chain head from a validator vote or block proposal.
    /// If a head with the same `block_hash` already exists, its stake weight
    /// is increased (saturating). Otherwise a new head is inserted.
    /// Old heads more than 100 slots behind the maximum are pruned.
    pub fn add_head(&mut self, slot: u64, block_hash: Hash, stake_weight: u64) {
        if let Some(head) = self.heads.iter_mut().find(|h| h.1 == block_hash) {
            head.2 = head.2.saturating_add(stake_weight);
        } else {
            self.heads.push((slot, block_hash, stake_weight));
        }
        // Prune old heads (keep only heads within 100 slots of max)
        let max_slot = self.heads.iter().map(|h| h.0).max().unwrap_or(0);
        self.heads.retain(|h| h.0 + 100 >= max_slot);
    }

    /// Select the canonical chain head.
    /// Priority: highest slot → most accumulated stake weight → deterministic hash tiebreak.
    pub fn select_head(&self) -> Option<(u64, Hash)> {
        self.heads
            .iter()
            .max_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1 .0.cmp(&b.1 .0)))
            .map(|(slot, hash, _)| (*slot, *hash))
    }

    /// Get all current chain heads
    pub fn heads(&self) -> &[(u64, Hash, u64)] {
        &self.heads
    }

    /// Clear all heads (e.g. after finalization)
    pub fn clear(&mut self) {
        self.heads.clear();
    }

    // ---- backward-compat convenience wrappers ----

    /// Add block weight (from votes) — legacy API, delegates to add_head with slot 0
    pub fn add_weight(&mut self, block_hash: Hash, weight: u64) {
        self.add_head(0, block_hash, weight);
    }

    /// Get weight of block
    pub fn get_weight(&self, block_hash: &Hash) -> u64 {
        self.heads
            .iter()
            .find(|h| h.1 == *block_hash)
            .map(|h| h.2)
            .unwrap_or(0)
    }

    /// Select best block from competing fork candidates (legacy API)
    pub fn select_best(&self, candidates: &[Hash]) -> Option<Hash> {
        candidates
            .iter()
            .filter_map(|hash| {
                self.heads
                    .iter()
                    .find(|h| h.1 == *hash)
                    .map(|h| (hash, h.2))
            })
            .max_by_key(|(_, w)| *w)
            .map(|(hash, _)| *hash)
    }

    /// Prune heads not in the keep list (legacy API)
    pub fn prune(&mut self, keep_hashes: &[Hash]) {
        self.heads.retain(|h| keep_hashes.contains(&h.1));
    }
}

// ============================================================================
// SLASHING - Byzantine Fault Punishment
// ============================================================================

/// Types of slashable offenses
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum SlashingOffense {
    /// Validator produced two different blocks for the same slot
    DoubleBlock {
        slot: u64,
        block_hash_1: Hash,
        block_hash_2: Hash,
    },
    /// Validator voted for two different blocks at the same slot
    DoubleVote {
        slot: u64,
        vote_1: Vote,
        vote_2: Vote,
    },
    /// Validator has been offline for extended period
    Downtime {
        last_active_slot: u64,
        current_slot: u64,
        missed_slots: u64,
    },
    /// Validator submitted an invalid state transition (100% stake loss per whitepaper)
    InvalidStateTransition { slot: u64, description: String },
    /// Validator deliberately censored transactions (25% stake loss per whitepaper)
    Censorship { slot: u64, censored_tx_count: u64 },
    /// Validator detected colluding with others (permanent ban per whitepaper)
    Collusion {
        slot: u64,
        colluding_validators: Vec<Pubkey>,
    },
}

/// Evidence of slashable behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingEvidence {
    /// The offense committed
    pub offense: SlashingOffense,
    /// Validator who committed the offense
    pub validator: Pubkey,
    /// Slot when evidence was created
    pub evidence_slot: u64,
    /// Validator who reported the evidence
    pub reporter: Pubkey,
    /// Timestamp
    pub timestamp: u64,
}

impl SlashingEvidence {
    /// Create new slashing evidence
    pub fn new(
        offense: SlashingOffense,
        validator: Pubkey,
        evidence_slot: u64,
        reporter: Pubkey,
    ) -> Self {
        SlashingEvidence {
            offense,
            validator,
            evidence_slot,
            reporter,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Verify evidence is valid
    pub fn verify(&self) -> bool {
        match &self.offense {
            SlashingOffense::DoubleBlock {
                slot: _,
                block_hash_1,
                block_hash_2,
            } => {
                // Must be same slot, different hashes
                block_hash_1 != block_hash_2
            }
            SlashingOffense::DoubleVote {
                slot: _,
                vote_1,
                vote_2,
            } => {
                // Must be same slot, different blocks, same validator, valid signatures
                vote_1.slot == vote_2.slot
                    && vote_1.block_hash != vote_2.block_hash
                    && vote_1.validator == vote_2.validator
                    && vote_1.validator == self.validator
                    && vote_1.verify()
                    && vote_2.verify()
            }
            SlashingOffense::Downtime {
                last_active_slot,
                current_slot,
                missed_slots,
            } => {
                // Verify downtime calculation
                *missed_slots == current_slot.saturating_sub(*last_active_slot) && *missed_slots > 0
            }
            SlashingOffense::InvalidStateTransition {
                slot: _,
                description,
            } => !description.is_empty(),
            SlashingOffense::Censorship {
                slot: _,
                censored_tx_count,
            } => *censored_tx_count > 0,
            SlashingOffense::Collusion {
                slot: _,
                colluding_validators,
            } => colluding_validators.len() >= 2,
        }
    }

    /// Get severity level (0-100)
    pub fn severity(&self) -> u64 {
        match &self.offense {
            SlashingOffense::DoubleBlock { .. } => 100, // Most severe - direct attack
            SlashingOffense::DoubleVote { .. } => 80,   // Severe - consensus violation
            SlashingOffense::Downtime { missed_slots, .. } => {
                // Scale from 10-50 based on downtime
                (10 + (missed_slots / 10).min(40)).min(50)
            }
            SlashingOffense::InvalidStateTransition { .. } => 100, // Maximum severity
            SlashingOffense::Censorship { .. } => 70,              // High severity
            SlashingOffense::Collusion { .. } => 100,              // Maximum - permanent ban
        }
    }
}

/// Slashing tracker - manages evidence and penalties
/// AUDIT-FIX 2.6: Made serializable for persistence to RocksDB
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingTracker {
    /// Evidence by validator
    evidence: std::collections::HashMap<Pubkey, Vec<SlashingEvidence>>,
    /// Slashed validators
    slashed: std::collections::HashSet<Pubkey>,
    /// Permanently banned validators (collusion = permanent ban per whitepaper)
    permanently_banned: std::collections::HashSet<Pubkey>,
}

impl Default for SlashingTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashingTracker {
    /// Create new slashing tracker
    pub fn new() -> Self {
        SlashingTracker {
            evidence: std::collections::HashMap::new(),
            slashed: std::collections::HashSet::new(),
            permanently_banned: std::collections::HashSet::new(),
        }
    }

    /// Add evidence of slashable behavior
    pub fn add_evidence(&mut self, evidence: SlashingEvidence) -> bool {
        // Verify evidence is valid
        if !evidence.verify() {
            return false;
        }

        // Add to evidence list
        let validator_evidence = self.evidence.entry(evidence.validator).or_default();

        // Check for duplicate evidence
        if validator_evidence
            .iter()
            .any(|e| match (&e.offense, &evidence.offense) {
                (
                    SlashingOffense::DoubleBlock { slot: s1, .. },
                    SlashingOffense::DoubleBlock { slot: s2, .. },
                ) => s1 == s2,
                (
                    SlashingOffense::DoubleVote { vote_1: v1, .. },
                    SlashingOffense::DoubleVote { vote_1: v2, .. },
                ) => v1.slot == v2.slot,
                (
                    SlashingOffense::InvalidStateTransition { slot: s1, .. },
                    SlashingOffense::InvalidStateTransition { slot: s2, .. },
                ) => s1 == s2,
                (
                    SlashingOffense::Censorship { slot: s1, .. },
                    SlashingOffense::Censorship { slot: s2, .. },
                ) => s1 == s2,
                (
                    SlashingOffense::Collusion { slot: s1, .. },
                    SlashingOffense::Collusion { slot: s2, .. },
                ) => s1 == s2,
                // M7 fix: deduplicate Downtime evidence by missed_slots
                (
                    SlashingOffense::Downtime {
                        missed_slots: m1, ..
                    },
                    SlashingOffense::Downtime {
                        missed_slots: m2, ..
                    },
                ) => m1 == m2,
                _ => false,
            })
        {
            return false; // Already have evidence for this offense
        }

        validator_evidence.push(evidence);
        true
    }

    /// Check if validator should be slashed
    pub fn should_slash(&self, validator: &Pubkey) -> bool {
        if let Some(evidence_list) = self.evidence.get(validator) {
            // Slash if any severe offense (severity >= 70 covers all whitepaper offenses)
            evidence_list.iter().any(|e| e.severity() >= 70)
        } else {
            false
        }
    }

    /// Mark validator as slashed
    pub fn slash(&mut self, validator: &Pubkey) -> bool {
        if self.should_slash(validator) {
            self.slashed.insert(*validator);
            // Check for collusion → permanent ban
            if let Some(evidence_list) = self.evidence.get(validator) {
                if evidence_list
                    .iter()
                    .any(|e| matches!(e.offense, SlashingOffense::Collusion { .. }))
                {
                    self.permanently_banned.insert(*validator);
                }
            }
            true
        } else {
            false
        }
    }

    /// Check if validator is slashed
    pub fn is_slashed(&self, validator: &Pubkey) -> bool {
        self.slashed.contains(validator)
    }

    /// Check if validator is permanently banned (collusion per whitepaper)
    pub fn is_permanently_banned(&self, validator: &Pubkey) -> bool {
        self.permanently_banned.contains(validator)
    }

    /// Get evidence for validator
    pub fn get_evidence(&self, validator: &Pubkey) -> Option<&Vec<SlashingEvidence>> {
        self.evidence.get(validator)
    }

    /// Calculate total penalty for validator
    pub fn calculate_penalty(&self, validator: &Pubkey) -> u64 {
        if let Some(evidence_list) = self.evidence.get(validator) {
            // Sum all severity scores
            evidence_list.iter().map(|e| e.severity()).sum()
        } else {
            0
        }
    }

    /// Apply economic slashing to stake pool (returns total amount slashed)
    pub fn apply_economic_slashing(
        &mut self,
        validator: &Pubkey,
        stake_pool: &mut StakePool,
    ) -> u64 {
        if !self.should_slash(validator) {
            return 0;
        }

        let mut total_slashed = 0u64;

        if let Some(evidence_list) = self.evidence.get(validator) {
            for evidence in evidence_list {
                // Calculate stake to slash based on severity
                let stake_penalty = match evidence.offense {
                    SlashingOffense::DoubleBlock { .. } => {
                        // Slash 50% of stake for double block production
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake() / 2)
                            .unwrap_or(0)
                    }
                    SlashingOffense::DoubleVote { .. } => {
                        // Slash 30% of stake for double voting
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake() * 30 / 100)
                            .unwrap_or(0)
                    }
                    SlashingOffense::Downtime { missed_slots, .. } => {
                        // Slash proportional to downtime (max 10%)
                        let downtime_penalty = (missed_slots / 100).min(10); // 1% per 100 slots, max 10%
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake() * downtime_penalty / 100)
                            .unwrap_or(0)
                    }
                    SlashingOffense::InvalidStateTransition { .. } => {
                        // Slash 100% of stake for invalid state transition (per whitepaper)
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake())
                            .unwrap_or(0)
                    }
                    SlashingOffense::Censorship { .. } => {
                        // Slash 25% of stake for censorship attack (per whitepaper)
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake() * 25 / 100)
                            .unwrap_or(0)
                    }
                    SlashingOffense::Collusion { .. } => {
                        // Slash 100% of stake + permanent ban (per whitepaper)
                        stake_pool
                            .get_stake(validator)
                            .map(|s| s.total_stake())
                            .unwrap_or(0)
                    }
                };

                if stake_penalty > 0 {
                    let slashed = stake_pool.slash_validator(validator, stake_penalty);
                    total_slashed += slashed;
                }
            }
        }

        if total_slashed > 0 {
            self.slash(validator);
        }

        total_slashed
    }

    /// Clear old evidence (older than given slot)
    pub fn prune_old_evidence(&mut self, current_slot: u64, keep_slots: u64) {
        let cutoff_slot = current_slot.saturating_sub(keep_slots);

        for evidence_list in self.evidence.values_mut() {
            evidence_list.retain(|e| e.evidence_slot >= cutoff_slot);
        }

        // Remove validators with no evidence
        self.evidence.retain(|_, list| !list.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validator_set() {
        let mut set = ValidatorSet::new();
        let pubkey1 = Pubkey::new([1u8; 32]);
        let pubkey2 = Pubkey::new([2u8; 32]);

        set.add_validator(ValidatorInfo::new(pubkey1, 0));
        set.add_validator(ValidatorInfo::new(pubkey2, 0));

        assert_eq!(set.validators().len(), 2);
        assert!(set.get_validator(&pubkey1).is_some());

        // Leader selection should be deterministic
        let leader1 = set.select_leader(0);
        let leader2 = set.select_leader(0);
        assert_eq!(leader1, leader2);
    }

    #[test]
    fn test_vote_aggregator() {
        let mut agg = VoteAggregator::new();
        let vote = Vote::new(
            1,
            Hash::new([0u8; 32]),
            Pubkey::new([1u8; 32]),
            [0u8; 64], // Dummy signature
        );

        // Note: Will fail verification, but tests structure
        agg.add_vote(vote.clone());

        assert_eq!(agg.vote_count(1, &Hash::new([0u8; 32])), 0); // Failed verification
    }

    #[test]
    fn test_reputation_updates() {
        let mut validator = ValidatorInfo::new(Pubkey::new([1u8; 32]), 0);
        let initial = validator.reputation;

        validator.update_reputation(true);
        assert!(validator.reputation > initial);

        validator.update_reputation(false);
        assert!(validator.reputation < initial + 10);
    }

    #[test]
    fn test_weighted_leader_selection_deterministic() {
        let mut set = ValidatorSet::new();
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);

        set.add_validator(ValidatorInfo::new(pk1, 0));
        set.add_validator(ValidatorInfo::new(pk2, 0));

        pool.stake(pk1, 100_000_000_000_000, 0).unwrap(); // 100k MOLT
        pool.stake(pk2, 150_000_000_000_000, 0).unwrap(); // 150k MOLT

        // Same slot → same leader every time
        let l1 = set.select_leader_weighted(10, &pool);
        let l2 = set.select_leader_weighted(10, &pool);
        assert_eq!(l1, l2);

        // Different slot may pick different leader
        let mut found_pk1 = false;
        let mut found_pk2 = false;
        for slot in 0..100 {
            match set.select_leader_weighted(slot, &pool) {
                Some(pk) if pk == pk1 => found_pk1 = true,
                Some(pk) if pk == pk2 => found_pk2 = true,
                _ => {}
            }
        }
        // With 2 validators and 100 slots, both should be selected at least once
        assert!(found_pk1, "pk1 should have been selected at least once");
        assert!(found_pk2, "pk2 should have been selected at least once");
    }

    #[test]
    fn test_weighted_falls_back_to_round_robin() {
        let mut set = ValidatorSet::new();
        let pool = StakePool::new(); // empty pool
        let pk1 = Pubkey::new([1u8; 32]);

        set.add_validator(ValidatorInfo::new(pk1, 0));

        // With empty stake pool (total_weight=0), should fall back to round-robin
        let leader = set.select_leader_weighted(0, &pool);
        assert_eq!(leader, Some(pk1));
    }

    #[test]
    fn test_is_leader_weighted() {
        let mut set = ValidatorSet::new();
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);

        set.add_validator(ValidatorInfo::new(pk1, 0));
        pool.stake(pk1, 100_000_000_000_000, 0).unwrap(); // 100k MOLT

        assert!(set.is_leader_weighted(0, &pk1, &pool));
    }

    #[test]
    fn test_slashing() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, 100_000_000_000_000, 0).unwrap(); // 100k MOLT

        let slashed = pool.slash_validator(&pk, 50_000_000_000_000); // slash 50k MOLT
        assert_eq!(slashed, 50_000_000_000_000);

        let remaining = pool.get_stake(&pk).map(|s| s.total_stake()).unwrap_or(0);
        assert_eq!(remaining, 50_000_000_000_000);
    }

    // ================================================================
    // T4.1 — ForkChoice tests
    // ================================================================

    #[test]
    fn test_fork_choice_select_highest_slot() {
        let mut fc = ForkChoice::new();
        let h1 = Hash::new([1u8; 32]);
        let h2 = Hash::new([2u8; 32]);

        fc.add_head(10, h1, 100);
        fc.add_head(20, h2, 50);

        let (slot, hash) = fc.select_head().unwrap();
        assert_eq!(slot, 20);
        assert_eq!(hash, h2);
    }

    #[test]
    fn test_fork_choice_tie_uses_stake() {
        let mut fc = ForkChoice::new();
        let h1 = Hash::new([1u8; 32]);
        let h2 = Hash::new([2u8; 32]);

        fc.add_head(20, h1, 100);
        fc.add_head(20, h2, 200);

        let (_, hash) = fc.select_head().unwrap();
        assert_eq!(hash, h2); // higher stake wins
    }

    #[test]
    fn test_fork_choice_accumulates_stake() {
        let mut fc = ForkChoice::new();
        let h1 = Hash::new([1u8; 32]);

        fc.add_head(10, h1, 50);
        fc.add_head(10, h1, 70);

        assert_eq!(fc.get_weight(&h1), 120);
    }

    #[test]
    fn test_fork_choice_prunes_old_heads() {
        let mut fc = ForkChoice::new();
        let old = Hash::new([1u8; 32]);
        let recent = Hash::new([2u8; 32]);

        fc.add_head(1, old, 100);
        fc.add_head(200, recent, 50);

        // old head at slot 1 should be pruned (200-1 > 100)
        assert_eq!(fc.heads().len(), 1);
        assert_eq!(fc.heads()[0].1, recent);
    }

    #[test]
    fn test_fork_choice_clear() {
        let mut fc = ForkChoice::new();
        fc.add_head(5, Hash::new([1u8; 32]), 10);
        fc.clear();
        assert!(fc.select_head().is_none());
    }

    #[test]
    fn test_fork_choice_legacy_select_best() {
        let mut fc = ForkChoice::new();
        let h1 = Hash::new([1u8; 32]);
        let h2 = Hash::new([2u8; 32]);
        let h3 = Hash::new([3u8; 32]);

        fc.add_weight(h1, 10);
        fc.add_weight(h2, 30);

        let best = fc.select_best(&[h1, h2, h3]);
        assert_eq!(best, Some(h2));
    }

    // ================================================================
    // T4.2 — Epoch boundary tests
    // ================================================================

    #[test]
    fn test_slot_to_epoch() {
        assert_eq!(slot_to_epoch(0), 0);
        assert_eq!(slot_to_epoch(SLOTS_PER_EPOCH - 1), 0);
        assert_eq!(slot_to_epoch(SLOTS_PER_EPOCH), 1);
        assert_eq!(slot_to_epoch(SLOTS_PER_EPOCH * 5 + 42), 5);
    }

    #[test]
    fn test_epoch_start_slot() {
        assert_eq!(epoch_start_slot(0), 0);
        assert_eq!(epoch_start_slot(1), SLOTS_PER_EPOCH);
        assert_eq!(epoch_start_slot(3), SLOTS_PER_EPOCH * 3);
    }

    #[test]
    fn test_is_epoch_boundary() {
        assert!(!is_epoch_boundary(0)); // slot 0 is not a boundary
        assert!(is_epoch_boundary(SLOTS_PER_EPOCH));
        assert!(is_epoch_boundary(SLOTS_PER_EPOCH * 7));
        assert!(!is_epoch_boundary(SLOTS_PER_EPOCH + 1));
    }

    #[test]
    fn test_epoch_info_for_slot() {
        let info = EpochInfo::for_slot(SLOTS_PER_EPOCH + 100, 5000, 3);
        assert_eq!(info.epoch, 1);
        assert_eq!(info.start_slot, SLOTS_PER_EPOCH);
        assert_eq!(info.end_slot, SLOTS_PER_EPOCH * 2 - 1);
        assert_eq!(info.total_stake, 5000);
        assert_eq!(info.validator_count, 3);
    }

    // ================================================================
    // T4.4 — Unstake cooldown (verify existing enforcement)
    // ================================================================

    #[test]
    fn test_claim_unstake_cooldown_enforced() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, 200_000_000_000_000, 0).unwrap(); // 200k MOLT

        // Graduate validator so they can unstake
        if let Some(si) = pool.stakes.get_mut(&pk) {
            si.status = BootstrapStatus::FullyVested;
            si.bootstrap_debt = 0;
        }

        pool.request_unstake(&pk, 50_000_000_000_000, 100, pk)
            .unwrap();

        // Claim too early → error
        let err = pool.claim_unstake(&pk, 200, &pk).unwrap_err();
        assert!(err.contains("Cooldown not complete"));

        // Claim after cooldown → success
        let amount = pool
            .claim_unstake(&pk, 100 + UNSTAKE_COOLDOWN_SLOTS, &pk)
            .unwrap();
        assert_eq!(amount, 50_000_000_000_000);
    }

    // ================================================================
    // T4.5 — Epoch reward distribution tests
    // ================================================================

    #[test]
    fn test_distribute_epoch_rewards_proportional() {
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);

        pool.stake(pk1, 100_000_000_000_000, 0).unwrap(); // 100k MOLT
        pool.stake(pk2, 300_000_000_000_000, 0).unwrap(); // 300k MOLT

        let rewards = pool.distribute_epoch_rewards(1_000_000);
        assert_eq!(rewards.len(), 2);

        // pk1 has 25% of total stake → 250k ; pk2 has 75% → 750k
        let r1 = rewards.iter().find(|(pk, _)| *pk == pk1).unwrap().1;
        let r2 = rewards.iter().find(|(pk, _)| *pk == pk2).unwrap().1;
        assert_eq!(r1, 250_000);
        assert_eq!(r2, 750_000);
    }

    #[test]
    fn test_distribute_epoch_rewards_empty() {
        let pool = StakePool::new();
        assert!(pool.distribute_epoch_rewards(1_000_000).is_empty());
    }

    #[test]
    fn test_distribute_epoch_rewards_zero_pool() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, 100_000_000_000_000, 0).unwrap(); // 100k MOLT
        assert!(pool.distribute_epoch_rewards(0).is_empty());
    }

    // ================================================================
    // T6.3 — Pending unstakes must not dilute active stakers
    // ================================================================

    #[test]
    fn test_pending_unstakes_excluded_from_rewards() {
        let mut pool = StakePool::new();
        let pk_a = Pubkey::new([1u8; 32]);
        let pk_b = Pubkey::new([2u8; 32]);
        pool.stake(pk_a, 200_000_000_000_000, 0).unwrap(); // 200k MOLT
        pool.stake(pk_b, 200_000_000_000_000, 0).unwrap(); // 200k MOLT

        assert_eq!(pool.total_stake(), 400_000_000_000_000);
        assert_eq!(pool.active_stake(), 400_000_000_000_000);
        assert_eq!(pool.pending_unstake_total(), 0);

        // Before unstake: equal 50/50 reward split
        let rewards = pool.distribute_epoch_rewards(1_000_000);
        let r_a = rewards.iter().find(|(pk, _)| *pk == pk_a).unwrap().1;
        let r_b = rewards.iter().find(|(pk, _)| *pk == pk_b).unwrap().1;
        assert_eq!(r_a, 500_000);
        assert_eq!(r_b, 500_000);

        // Graduate validator A so they can unstake
        if let Some(si) = pool.stakes.get_mut(&pk_a) {
            si.status = BootstrapStatus::FullyVested;
            si.bootstrap_debt = 0;
        }

        // A requests unstake of 100k (keeping 100k active)
        pool.request_unstake(&pk_a, 100_000_000_000_000, 100, pk_a)
            .unwrap();

        // Verify: total active stake is 300k (A=100k, B=200k), 100k pending
        assert_eq!(pool.active_stake(), 300_000_000_000_000);
        assert_eq!(pool.pending_unstake_total(), 100_000_000_000_000);

        // After unstake: A gets 1/3, B gets 2/3 — pending unstake does NOT dilute B
        let rewards2 = pool.distribute_epoch_rewards(900_000);
        let r_a2 = rewards2.iter().find(|(pk, _)| *pk == pk_a).unwrap().1;
        let r_b2 = rewards2.iter().find(|(pk, _)| *pk == pk_b).unwrap().1;
        assert_eq!(r_a2, 300_000); // 100k/300k = 1/3
        assert_eq!(r_b2, 600_000); // 200k/300k = 2/3
    }

    #[test]
    fn test_pending_unstake_total_tracking() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, 200_000_000_000_000, 0).unwrap(); // 200k MOLT

        // Graduate
        if let Some(si) = pool.stakes.get_mut(&pk) {
            si.status = BootstrapStatus::FullyVested;
            si.bootstrap_debt = 0;
        }

        assert_eq!(pool.pending_unstake_total(), 0);
        pool.request_unstake(&pk, 50_000_000_000_000, 100, pk)
            .unwrap();
        assert_eq!(pool.pending_unstake_total(), 50_000_000_000_000);

        // Claim after cooldown clears the pending amount
        pool.claim_unstake(&pk, 100 + UNSTAKE_COOLDOWN_SLOTS, &pk)
            .unwrap();
        assert_eq!(pool.pending_unstake_total(), 0);
    }

    #[test]
    fn test_voting_power_excludes_pending_unstakes() {
        let mut pool = StakePool::new();
        let pk_a = Pubkey::new([1u8; 32]);
        let pk_b = Pubkey::new([2u8; 32]);
        pool.stake(pk_a, 200_000_000_000_000, 0).unwrap(); // 200k MOLT
        pool.stake(pk_b, 200_000_000_000_000, 0).unwrap(); // 200k MOLT

        // Initially: equal voting power
        let vp_a = pool.voting_power(&pk_a);
        let vp_b = pool.voting_power(&pk_b);
        assert_eq!(vp_a, vp_b);
        assert_eq!(vp_a, 5000); // 50% * 10000

        // Graduate A and unstake half
        if let Some(si) = pool.stakes.get_mut(&pk_a) {
            si.status = BootstrapStatus::FullyVested;
            si.bootstrap_debt = 0;
        }
        pool.request_unstake(&pk_a, 100_000_000_000_000, 100, pk_a)
            .unwrap();

        // A now has 100k/300k = 33.3%, B has 200k/300k = 66.6%
        let vp_a2 = pool.voting_power(&pk_a);
        let vp_b2 = pool.voting_power(&pk_b);
        assert_eq!(vp_a2, 3333); // ~33.3%
        assert_eq!(vp_b2, 6666); // ~66.6%
    }
}
