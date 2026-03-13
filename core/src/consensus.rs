// MoltChain Consensus Module
// Byzantine Fault Tolerant consensus with Proof of Contribution

use crate::contract::ContractAccount;
use crate::genesis::ConsensusParams;
use crate::{Block, Hash, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// STAKING - Economic Security
// ============================================================================

/// Minimum stake required to remain an active validator (75,000 MOLT — $7,500 at $0.10/MOLT).
/// Bootstrap grant is still 100K MOLT, but validators stay active down to 75K,
/// giving a 25% buffer before deactivation after slashing.
pub const MIN_VALIDATOR_STAKE: u64 = 75_000 * 1_000_000_000; // 75k MOLT in shells

/// Bootstrap grant amount (100,000 MOLT) — the initial stake granted to the first 200 validators
pub const BOOTSTRAP_GRANT_AMOUNT: u64 = 100_000 * 1_000_000_000; // 100k MOLT in shells

/// Transaction block reward (0.02 MOLT per block with transactions)
/// Reduced from 0.1 MOLT for BFT adaptive timing (~200ms tx blocks, ~5× faster).
/// At ~90,000 blocks/day with 3 validators, each earns ~30,000 blocks × 0.02 = 600 MOLT/day.
pub const TRANSACTION_BLOCK_REWARD: u64 = 20_000_000; // 0.02 MOLT

/// Heartbeat block reward (0.01 MOLT per heartbeat — 50% of transaction reward)
/// Reduced from 0.05 MOLT for BFT adaptive timing (~800ms heartbeats, ~5× faster).
pub const HEARTBEAT_BLOCK_REWARD: u64 = 10_000_000; // 0.01 MOLT

/// Legacy constant for backward compatibility (uses transaction reward)
pub const BLOCK_REWARD: u64 = TRANSACTION_BLOCK_REWARD;

/// Target annual reward pool draw rate (informational only)
/// MOLT is NOT inflationary — rewards are drawn from the validator rewards pool.
/// With 0.02/0.01 MOLT rewards + 20% annual decay, the pool lasts 200+ years.
pub const ANNUAL_REWARD_RATE_BPS: u64 = 500;

/// Slots per year (assuming 400ms per slot = ~78.8M slots/year)
pub const SLOTS_PER_YEAR: u64 = 78_840_000;

/// Annual reward decay rate in basis points (2000 = 20%).
/// Block rewards decrease by 20% per year since genesis, computed deterministically.
/// Year 0: 100%, Year 1: 80%, Year 5: 32.8%, Year 10: 10.7%, Year 50: ~0%.
pub const ANNUAL_REWARD_DECAY_BPS: u64 = 2000;

// ============================================================================
// FOUNDING MOLTYS VESTING
// ============================================================================

/// Founding moltys cliff period: 6 months in seconds (6 × 30 × 86400).
/// No tokens unlock until this duration has elapsed since genesis.
pub const FOUNDING_CLIFF_SECONDS: u64 = 6 * 30 * 24 * 3600; // 15,552,000

/// Founding moltys total vest duration: 24 months in seconds (24 × 30 × 86400).
/// The linear vest runs from month 6 to month 24 (18 months of linear unlock).
pub const FOUNDING_VEST_TOTAL_SECONDS: u64 = 24 * 30 * 24 * 3600; // 62,208,000

/// Compute the cumulative amount of founding moltys that should be unlocked
/// at `current_time` (Unix seconds).
///
/// Schedule: 6-month cliff, then 18-month linear vest (to month 24).
///   - Before cliff_end: 0
///   - Between cliff_end and vest_end: linear proportion
///   - At or after vest_end: total_amount (fully vested)
///
/// `cliff_end` and `vest_end` are absolute Unix timestamps computed at genesis:
///   cliff_end = genesis_time + FOUNDING_CLIFF_SECONDS
///   vest_end  = genesis_time + FOUNDING_VEST_TOTAL_SECONDS
pub fn founding_vesting_unlocked(
    total_amount: u64,
    cliff_end: u64,
    vest_end: u64,
    current_time: u64,
) -> u64 {
    if current_time < cliff_end {
        return 0;
    }
    if current_time >= vest_end {
        return total_amount;
    }
    let linear_period = vest_end - cliff_end;
    if linear_period == 0 {
        return total_amount;
    }
    let elapsed = current_time - cliff_end;
    // Use u128 to avoid overflow on large amounts × elapsed
    (total_amount as u128 * elapsed as u128 / linear_period as u128) as u64
}

/// Compute the decayed block reward for a given slot.
///
/// Applies compound 20% annual decay: reward_year_n = base × (80/100)^n.
/// Genesis is slot 0, so `current_slot` IS `slots_since_genesis`.
/// Capped at 50 iterations (reward is effectively 0 past year 50).
///
/// # Examples
/// ```
/// use moltchain_core::consensus::decayed_reward;
/// assert_eq!(decayed_reward(100_000_000, 0), 100_000_000); // year 0
/// assert_eq!(decayed_reward(100_000_000, 78_840_000), 80_000_000); // year 1
/// ```
pub fn decayed_reward(base_reward: u64, current_slot: u64) -> u64 {
    let years = current_slot / SLOTS_PER_YEAR;
    let mut reward = base_reward;
    // AUDIT-FIX L2: 20% decay per year → multiply by 80/100 each year
    // Extended from 50 to 200 cap (reward reaches 0 by ~80 years anyway)
    for _ in 0..years.min(200) {
        reward = reward * 80 / 100;
        if reward == 0 {
            break;
        }
    }
    reward
}

// ============================================================================
// GRADUATION - Bootstrap-to-Maturity System
// ============================================================================

/// Maximum number of validators that receive the bootstrap grant (first 200)
pub const MAX_BOOTSTRAP_VALIDATORS: u64 = 200;

/// Maximum bootstrap duration in slots (~18 months = 547 days × 216,000 slots/day)
pub const MAX_BOOTSTRAP_SLOTS: u64 = 547 * 216_000; // 118,152,000 slots

/// Uptime threshold for performance bonus (95% = 9500 basis points)
pub const UPTIME_BONUS_THRESHOLD_BPS: u64 = 9500;

/// Performance bonus multiplier (1.5× = 15000 basis points, i.e. 75/25 split)
pub const PERFORMANCE_BONUS_BPS: u64 = 15000;

/// Cooldown period after machine migration before fingerprint slot is released
/// (1 epoch = 432,000 slots ≈ 2 days)
pub const MIGRATION_COOLDOWN_SLOTS: u64 = 432_000;

/// Oracle price staleness threshold (1 hour in seconds — matches moltoracle contract)
const ORACLE_STALENESS_SECS: u64 = 3600;

// ============================================================================
// PRICE-BASED REWARDS - Dynamic reward adjustment
// ============================================================================

/// Price oracle interface for on-chain price feeds
pub trait PriceOracle: Send + Sync {
    fn get_molt_price_usd(&self) -> f64;
}

/// On-chain oracle that reads price data from the moltoracle contract's storage.
/// Falls back to the reference price ($0.10) if the oracle has no data or is stale.
pub struct StateOracle {
    state: Arc<crate::state::StateStore>,
}

impl StateOracle {
    pub fn new(state: Arc<crate::state::StateStore>) -> Self {
        Self { state }
    }

    /// Read the raw MOLT price feed from moltoracle contract storage.
    /// Returns (price_raw, decimals, timestamp) or None if unavailable.
    #[allow(dead_code)]
    fn read_molt_price_feed(&self) -> Option<(u64, u8, u64)> {
        read_molt_price_feed_from_state(&self.state)
    }
}

impl PriceOracle for StateOracle {
    fn get_molt_price_usd(&self) -> f64 {
        molt_price_from_state(&self.state)
    }
}

/// Read the raw MOLT price feed from moltoracle contract storage.
/// Returns (price_raw, decimals, timestamp) or None if unavailable.
pub fn read_molt_price_feed_from_state(state: &crate::state::StateStore) -> Option<(u64, u8, u64)> {
    // Resolve moltoracle program address via symbol registry
    let entry = state.get_symbol_registry("moltoracle").ok()??;
    let account = state.get_account(&entry.program).ok()??;
    let contract: ContractAccount = serde_json::from_slice(&account.data).ok()?;

    // Read "price_MOLT" feed — 49 bytes: price(8) + timestamp(8) + decimals(1) + feeder(32)
    let feed = contract.get_storage(b"price_MOLT")?;
    if feed.len() < 17 {
        return None;
    }

    let price_raw = u64::from_le_bytes(feed[0..8].try_into().ok()?);
    let timestamp = u64::from_le_bytes(feed[8..16].try_into().ok()?);
    let decimals = feed[16];

    Some((price_raw, decimals, timestamp))
}

/// Read the current MOLT price in USD from on-chain moltoracle storage.
/// Falls back to $0.10 (launch reference price) if oracle data is unavailable or stale.
pub fn molt_price_from_state(state: &crate::state::StateStore) -> f64 {
    match read_molt_price_feed_from_state(state) {
        Some((price_raw, decimals, timestamp)) => {
            // A-5: Use deterministic block timestamp instead of SystemTime::now()
            // This ensures all validators evaluating the same slot reach identical results.
            let now = state
                .get_last_slot()
                .ok()
                .and_then(|slot| state.get_block_by_slot(slot).ok().flatten())
                .map(|block| block.header.timestamp)
                .unwrap_or(0);

            if now > 0 && timestamp > 0 && now.saturating_sub(timestamp) > ORACLE_STALENESS_SECS {
                return 0.10; // Stale — fall back to reference price
            }

            if price_raw == 0 || decimals > 18 {
                return 0.10; // Invalid data
            }

            // Convert to f64 USD price
            let divisor = 10u64.pow(decimals as u32) as f64;
            let price = price_raw as f64 / divisor;

            // Sanity bounds: reject obviously wrong prices
            if !(0.000001..=1_000_000.0).contains(&price) {
                return 0.10;
            }

            price
        }
        None => 0.10, // No oracle data — use reference launch price
    }
}

/// Reward configuration with price-based adjustment
#[derive(Debug, Clone)]
pub struct RewardConfig {
    /// Base transaction reward (0.02 MOLT)
    pub base_transaction_reward: u64,
    /// Base heartbeat reward (0.01 MOLT)
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
/// Uses u64::is_multiple_of (stable since Rust 1.73)
pub fn is_epoch_boundary(slot: u64) -> bool {
    slot > 0 && slot.is_multiple_of(SLOTS_PER_EPOCH)
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
    /// SHA-256 machine fingerprint (platform UUID + MAC) — [0u8; 32] if not set
    #[serde(default)]
    pub machine_fingerprint: [u8; 32],
    /// Slot when this validator first staked (for time-cap graduation)
    #[serde(default)]
    pub start_slot: u64,
    /// Bootstrap grant index (0-based). Validators #0..199 get the grant; 200+ do not.
    /// u64::MAX means "not a bootstrap validator" (self-funded).
    #[serde(default = "default_no_bootstrap_index")]
    pub bootstrap_index: u64,
    /// Last slot when machine fingerprint was migrated (for migration cooldown)
    #[serde(default)]
    pub last_migration_slot: u64,
    /// If > 0, penalty repayment boost is active until this slot.
    /// While active, 90% of rewards go to debt repayment and only 10% are liquid.
    /// Set by the slashing sweep when a tier-2 downtime offense occurs.
    #[serde(default)]
    pub penalty_boost_until: u64,
}

/// Serde default for bootstrap_index — u64::MAX means "not a bootstrap validator"
fn default_no_bootstrap_index() -> u64 {
    u64::MAX
}

impl StakeInfo {
    /// Create new validator stake with bootstrap (Contributory Stake system).
    ///
    /// `bootstrap_index` — the grant sequence number (0..199 = bootstrap grant,
    /// 200+ or u64::MAX = self-funded, no debt).
    pub fn new(validator: Pubkey, amount: u64, current_slot: u64) -> Self {
        Self::with_bootstrap_index(validator, amount, current_slot, u64::MAX)
    }

    /// Create new validator stake, explicitly setting the bootstrap index.
    /// Validators with index < MAX_BOOTSTRAP_VALIDATORS receive bootstrap debt;
    /// others (index >= MAX_BOOTSTRAP_VALIDATORS or u64::MAX) are self-funded.
    pub fn with_bootstrap_index(
        validator: Pubkey,
        amount: u64,
        current_slot: u64,
        bootstrap_index: u64,
    ) -> Self {
        // Only bootstrap if this is one of the first 200 AND amount matches BOOTSTRAP_GRANT_AMOUNT
        let is_bootstrap =
            bootstrap_index < MAX_BOOTSTRAP_VALIDATORS && amount == BOOTSTRAP_GRANT_AMOUNT;
        let bootstrap_debt = if is_bootstrap { amount } else { 0 };

        Self {
            validator,
            amount,
            earned_amount: 0,
            bootstrap_debt,
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
            machine_fingerprint: [0u8; 32],
            start_slot: current_slot,
            bootstrap_index,
            last_migration_slot: 0,
            penalty_boost_until: 0,
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
    /// Slashing burns principal stake but does NOT reduce bootstrap debt.
    /// This prevents the perverse incentive where slashing would accelerate
    /// graduation by reducing debt proportionally. Only earned_amount is
    /// reduced to reflect the smaller principal.
    pub fn slash(&mut self, amount: u64) -> u64 {
        let slashed = amount.min(self.amount);
        if slashed == 0 || self.amount == 0 {
            return 0;
        }
        // Reduce principal
        let new_amount = self.amount.saturating_sub(slashed);
        // Scale earned_amount proportionally (it reflects real earned value)
        let ratio_num = new_amount;
        let ratio_den = self.amount;
        self.earned_amount =
            (self.earned_amount as u128 * ratio_num as u128 / ratio_den as u128) as u64;
        // IMPORTANT: Do NOT reduce bootstrap_debt — slashing should not help graduation.
        // The validator still owes the full original debt amount.
        self.amount = new_amount;
        self.is_active = self.meets_minimum();
        slashed
    }

    /// Add block reward to accumulated rewards
    /// AUDIT-FIX 1.2a: saturating_add to prevent overflow
    pub fn add_reward(&mut self, reward: u64, slot: u64) {
        self.rewards_earned = self.rewards_earned.saturating_add(reward);
        self.last_reward_slot = slot;
    }

    /// Claim accumulated rewards with Contributory Stake split.
    ///
    /// **Graduation paths:**
    /// 1. **Debt repaid** — bootstrap_debt reaches 0 (standard graduation).
    /// 2. **Time cap** — 18 months (MAX_BOOTSTRAP_SLOTS) elapsed since start_slot.
    ///
    /// **Performance bonus:**
    /// Validators with ≥ 95% uptime get a 75/25 split (75% to debt) instead of 50/50,
    /// accelerating graduation by ~1.5×.
    ///
    /// Returns (liquid_amount, debt_payment)
    ///
    /// `num_validators` — count of active validators in the stake pool.
    /// Used to compute the expected block production rate for the uptime formula:
    ///   expected_blocks = (current_slot - start_slot) / num_validators
    ///   uptime_bps = blocks_produced × 10000 / expected_blocks
    pub fn claim_rewards(&mut self, current_slot: u64, num_validators: u64) -> (u64, u64) {
        let total_reward = self.rewards_earned;
        self.rewards_earned = 0;

        if self.bootstrap_debt > 0 {
            // ── Time-cap graduation check ──────────────────────────
            // If 18 months have passed, forgive remaining debt immediately.
            if current_slot >= self.start_slot.saturating_add(MAX_BOOTSTRAP_SLOTS) {
                self.bootstrap_debt = 0;
                self.status = BootstrapStatus::FullyVested;
                self.graduation_slot = Some(current_slot);
                self.penalty_boost_until = 0; // Clear boost on graduation
                                              // All reward is liquid after time-cap graduation
                self.total_claimed = self.total_claimed.saturating_add(total_reward);
                return (total_reward, 0);
            }

            // ── Penalty repayment boost: 90% to debt, 10% liquid ──
            // Active when slashing sweep sets penalty_boost_until > current_slot.
            // This overrides the normal 50/75% split as punitive acceleration.
            let debt_fraction =
                if self.penalty_boost_until > 0 && current_slot < self.penalty_boost_until {
                    (total_reward as u128 * 90 / 100) as u64
                } else {
                    // Clear expired boost
                    if self.penalty_boost_until > 0 && current_slot >= self.penalty_boost_until {
                        self.penalty_boost_until = 0;
                    }

                    // ── Performance bonus: 95%+ uptime → accelerated repayment ──
                    if self.uptime_bps(current_slot, num_validators) >= UPTIME_BONUS_THRESHOLD_BPS {
                        let base_half = total_reward / 2;
                        (base_half as u128 * PERFORMANCE_BONUS_BPS as u128 / 10000) as u64
                    } else {
                        // Standard 50% to debt repayment
                        total_reward / 2
                    }
                };

            // Apply debt payment (capped at remaining debt)
            let paid = debt_fraction.min(self.bootstrap_debt);
            self.bootstrap_debt -= paid;
            // AUDIT-FIX 1.2b: saturating_add to prevent overflow
            self.earned_amount = self.earned_amount.saturating_add(paid);
            self.total_debt_repaid = self.total_debt_repaid.saturating_add(paid);

            // Liquid = everything not going to debt (includes excess if debt < fraction)
            let liquid = total_reward - paid;

            // Track total claimed (liquid + debt payment = full reward)
            self.total_claimed = self.total_claimed.saturating_add(total_reward);

            // Check for graduation
            if self.bootstrap_debt == 0 {
                self.status = BootstrapStatus::FullyVested;
                self.graduation_slot = Some(current_slot);
                // AUDIT-FIX LOW-13: Clear penalty boost on normal graduation too
                // (not just time-cap). Once debt is zero the boost has no effect
                // (takes the other code path), but clearing is cleaner.
                self.penalty_boost_until = 0;
            }

            (liquid, paid) // (spendable, locked_for_debt)
        } else {
            // Fully vested: 100% liquid
            self.total_claimed = self.total_claimed.saturating_add(total_reward);
            (total_reward, 0)
        }
    }

    /// Calculate validator uptime in basis points (0–10000).
    ///
    /// Per VALIDATOR_GRADUATION_PLAN.md §3.2:
    ///   expected_blocks = (current_slot - start_slot) / num_validators
    ///   uptime_bps = min(10000, blocks_produced × 10000 / expected_blocks)
    ///
    /// This accounts for the fact that with N validators, each one is expected
    /// to produce roughly 1/N of the total blocks. Without this factor, 95%
    /// uptime is trivially achievable regardless of actual availability.
    ///
    /// `num_validators` must be ≥ 1 (clamped internally).
    pub fn uptime_bps(&self, current_slot: u64, num_validators: u64) -> u64 {
        let slots_active = current_slot.saturating_sub(self.start_slot);
        if slots_active == 0 {
            return 0;
        }
        let num_val = num_validators.max(1);
        // Expected blocks this validator should have produced:
        // total_slots / num_validators (their fair share of block production)
        let expected_blocks = slots_active / num_val;
        if expected_blocks == 0 {
            // Not enough slots elapsed for even 1 expected block per validator
            // — give benefit of the doubt if they produced at least 1
            return if self.blocks_produced > 0 { 10000 } else { 0 };
        }
        let bps = self.blocks_produced.saturating_mul(10000) / expected_blocks;
        bps.min(10000)
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
        // AUDIT-FIX M8: u128 intermediate for overflow safety
        (self.earned_amount as u128 * 100 / total_bootstrap as u128) as u64
    }

    /// Calculate staking APY based on current stake, total staked, and current slot.
    ///
    /// Applies the 20% annual reward decay to the block reward before computing APY.
    /// `current_slot` is used to determine how many years of decay have elapsed.
    pub fn calculate_apy(&self, total_staked: u64, current_slot: u64) -> f64 {
        if total_staked == 0 {
            return 0.0;
        }
        // APY = (annual_inflation / total_staked) * 100
        // Higher stake concentration = lower individual APY
        // AUDIT-FIX 3.3: APY is display-only (not consensus-critical), f64 is acceptable
        // AUDIT-FIX M8: u128 intermediate before f64 cast
        let current_reward = decayed_reward(BLOCK_REWARD, current_slot);
        let annual_rewards = (current_reward as u128 * SLOTS_PER_YEAR as u128) as f64;
        (annual_rewards / total_staked as f64) * 100.0
    }

    /// Add additional stake (after graduation, up to 1M max)
    pub fn add_stake(&mut self, additional: u64) -> Result<(), String> {
        if !self.is_fully_vested() {
            return Err("Must be fully vested to add additional stake".to_string());
        }

        if additional > MAX_VALIDATOR_STAKE.saturating_sub(self.amount) {
            return Err(format!(
                "Cannot exceed maximum stake of {} MOLT",
                MAX_VALIDATOR_STAKE / 1_000_000_000
            ));
        }

        self.amount = self.amount.saturating_add(additional);
        Ok(())
    }

    /// Top up stake after being slashed — recovery mechanism.
    /// Unlike `add_stake()`, this does NOT require full vesting.
    /// Allows a slashed validator to add funds to get back above MIN_VALIDATOR_STAKE.
    /// Returns Ok(new_total) or Err if amount exceeds max or is zero.
    pub fn top_up_stake(&mut self, additional: u64) -> Result<u64, String> {
        if additional == 0 {
            return Err("Top-up amount must be greater than zero".to_string());
        }

        if additional > MAX_VALIDATOR_STAKE.saturating_sub(self.amount) {
            return Err(format!(
                "Cannot exceed maximum stake of {} MOLT",
                MAX_VALIDATOR_STAKE / 1_000_000_000
            ));
        }

        self.amount = self.amount.saturating_add(additional);
        self.is_active = self.meets_minimum();
        Ok(self.amount)
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

/// Custom serde for HashMap<[u8; 32], Pubkey> — JSON requires string keys.
/// Keys are hex-encoded for serialization, decoded on deserialization.
mod fingerprint_serde {
    use super::*;
    use serde::de::{self, Deserializer, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};
    use std::fmt;

    pub fn serialize<S>(map: &HashMap<[u8; 32], Pubkey>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut ser_map = serializer.serialize_map(Some(map.len()))?;
        for (key, value) in map {
            ser_map.serialize_entry(&hex::encode(key), value)?;
        }
        ser_map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HashMap<[u8; 32], Pubkey>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FpVisitor;

        impl<'de> Visitor<'de> for FpVisitor {
            type Value = HashMap<[u8; 32], Pubkey>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map of hex-encoded fingerprints to pubkeys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = HashMap::new();
                while let Some((key_str, value)) = map.next_entry::<String, Pubkey>()? {
                    let bytes = hex::decode(&key_str)
                        .map_err(|e| de::Error::custom(format!("bad hex fingerprint: {}", e)))?;
                    if bytes.len() != 32 {
                        return Err(de::Error::custom(format!(
                            "fingerprint must be 32 bytes, got {}",
                            bytes.len()
                        )));
                    }
                    let mut fp = [0u8; 32];
                    fp.copy_from_slice(&bytes);
                    result.insert(fp, value);
                }
                Ok(result)
            }
        }

        deserializer.deserialize_map(FpVisitor)
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
    /// Machine fingerprint registry: fingerprint → validator pubkey.
    /// Prevents the same physical machine from running multiple validators.
    #[serde(
        default,
        serialize_with = "fingerprint_serde::serialize",
        deserialize_with = "fingerprint_serde::deserialize"
    )]
    fingerprint_registry: HashMap<[u8; 32], Pubkey>,
    /// Number of bootstrap grants issued so far (monotonically increasing, 0..200)
    #[serde(default)]
    bootstrap_grants_issued: u64,
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
            fingerprint_registry: HashMap::new(),
            bootstrap_grants_issued: 0,
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

        // AUDIT-FIX A-2: Enforce MAX_VALIDATOR_STAKE cap
        if let Some(existing) = self.stakes.get(&validator) {
            let new_total = existing.amount.saturating_add(amount);
            if new_total > MAX_VALIDATOR_STAKE {
                return Err(format!(
                    "Stake {} would exceed max {} per validator",
                    new_total, MAX_VALIDATOR_STAKE
                ));
            }
        } else if amount > MAX_VALIDATOR_STAKE {
            return Err(format!(
                "Stake {} exceeds max {} per validator",
                amount, MAX_VALIDATOR_STAKE
            ));
        }

        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            stake_info.amount = stake_info.amount.saturating_add(amount);
            stake_info.is_active = stake_info.meets_minimum();
        } else {
            let stake_info = StakeInfo::new(validator, amount, current_slot);
            self.stakes.insert(validator, stake_info);
        }

        self.total_staked = self.total_staked.saturating_add(amount);
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

    /// Full-fidelity upsert: merge a complete StakeInfo from a remote snapshot.
    /// For existing entries, syncs bootstrap debt, vesting status, earned amounts,
    /// and fingerprint data — only if the remote has more progress (higher debt_repaid
    /// or higher earned_amount). For new entries, inserts the full StakeInfo as-is.
    pub fn upsert_stake_full(&mut self, entry: StakeInfo) {
        if let Some(local) = self.stakes.get_mut(&entry.validator) {
            let old_amount = local.amount;
            // Accept higher stake amount
            if entry.amount > old_amount {
                self.total_staked = self.total_staked.saturating_add(entry.amount - old_amount);
                local.amount = entry.amount;
            }
            // Sync bootstrap fields if remote has more progress
            if entry.total_debt_repaid > local.total_debt_repaid {
                local.bootstrap_debt = entry.bootstrap_debt;
                local.total_debt_repaid = entry.total_debt_repaid;
                local.earned_amount = entry.earned_amount;
                local.status = entry.status.clone();
                local.graduation_slot = entry.graduation_slot;
            }
            // Accept bootstrap_index if local is unset (u64::MAX)
            if local.bootstrap_index == u64::MAX && entry.bootstrap_index != u64::MAX {
                local.bootstrap_index = entry.bootstrap_index;
                // If we just learned this is a bootstrap validator, set debt if missing
                if local.bootstrap_debt == 0 && entry.bootstrap_debt > 0 {
                    local.bootstrap_debt = entry.bootstrap_debt;
                    local.status = entry.status.clone();
                }
            }
            // Sync fingerprint if local is empty
            if local.machine_fingerprint == [0u8; 32] && entry.machine_fingerprint != [0u8; 32] {
                local.machine_fingerprint = entry.machine_fingerprint;
            }
            // Accept higher blocks produced count
            if entry.blocks_produced > local.blocks_produced {
                local.blocks_produced = entry.blocks_produced;
            }
            // AUDIT-FIX MEDIUM-6: Sync penalty_boost_until field.
            // Accept the higher (later-expiring) penalty boost so that the
            // punitive repayment window is not lost during stake sync.
            if entry.penalty_boost_until > local.penalty_boost_until {
                local.penalty_boost_until = entry.penalty_boost_until;
            }
            local.is_active = local.meets_minimum();
        } else {
            // New entry — insert as-is with full fidelity
            self.total_staked = self.total_staked.saturating_add(entry.amount);
            let validator = entry.validator;
            // Sync fingerprint registry
            if entry.machine_fingerprint != [0u8; 32] {
                self.fingerprint_registry
                    .insert(entry.machine_fingerprint, validator);
            }
            // Track bootstrap grants
            if entry.bootstrap_index != u64::MAX
                && entry.bootstrap_index >= self.bootstrap_grants_issued
            {
                self.bootstrap_grants_issued = entry.bootstrap_index + 1;
            }
            self.stakes.insert(validator, entry);
        }
    }

    /// Slash validator stake (returns amount slashed)
    pub fn slash_validator(&mut self, validator: &Pubkey, amount: u64) -> u64 {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            let slashed = stake_info.slash(amount);
            self.total_staked = self.total_staked.saturating_sub(slashed);
            self.total_slashed = self.total_slashed.saturating_add(slashed);
            slashed
        } else {
            0
        }
    }

    /// Top up a validator's stake after slashing (recovery mechanism).
    /// Unlike `stake()`, this does not require meeting MIN_VALIDATOR_STAKE upfront.
    /// The validator must already exist in the stake pool.
    /// Returns Ok(new_total) or Err if validator not found or amount invalid.
    pub fn top_up_stake(&mut self, validator: &Pubkey, amount: u64) -> Result<u64, String> {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            let new_total = stake_info.top_up_stake(amount)?;
            self.total_staked = self.total_staked.saturating_add(amount);
            Ok(new_total)
        } else {
            Err("Validator not found in stake pool".to_string())
        }
    }

    /// Remove inactive validators that have been deactivated (stake below minimum)
    /// for more than `grace_slots` slots. Returns the list of removed pubkeys.
    /// Ghost validators (slashed & inactive) are cleaned up by this method.
    pub fn remove_ghost_validators(&mut self, current_slot: u64, grace_slots: u64) -> Vec<Pubkey> {
        let mut removed = Vec::new();
        let to_remove: Vec<Pubkey> = self
            .stakes
            .iter()
            .filter(|(_, info)| {
                // Only remove validators that are:
                // 1. Inactive (below minimum stake)
                // 2. Have been inactive long enough (beyond grace period)
                // 3. Have zero amount (fully slashed) OR have been below minimum for grace_slots
                !info.is_active
                    && info.amount == 0
                    && current_slot.saturating_sub(info.last_reward_slot) > grace_slots
            })
            .map(|(pk, _)| *pk)
            .collect();

        for pk in to_remove {
            if let Some(info) = self.stakes.remove(&pk) {
                self.total_staked = self.total_staked.saturating_sub(info.amount);
                // Clean up fingerprint registry
                if info.machine_fingerprint != [0u8; 32] {
                    self.fingerprint_registry.remove(&info.machine_fingerprint);
                }
                removed.push(pk);
            }
        }
        removed
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

    /// Distribute block reward to validator, with 20% annual decay applied.
    ///
    /// The base reward (TX or heartbeat) is decayed by 20% per year since genesis.
    /// Genesis is slot 0, so `slot` is used directly as `slots_since_genesis`.
    pub fn distribute_block_reward(
        &mut self,
        validator: &Pubkey,
        slot: u64,
        is_heartbeat: bool,
    ) -> u64 {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            if stake_info.is_active {
                let base = if is_heartbeat {
                    HEARTBEAT_BLOCK_REWARD
                } else {
                    TRANSACTION_BLOCK_REWARD
                };
                let reward = decayed_reward(base, slot);
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
    ///
    /// Automatically computes the active validator count from the stake pool
    /// and passes it to StakeInfo::claim_rewards for correct uptime calculation.
    pub fn claim_rewards(&mut self, validator: &Pubkey, current_slot: u64) -> (u64, u64) {
        // Count active validators for uptime formula:
        // expected_blocks = slots_active / num_active_validators
        let num_active: u64 = self.stakes.values().filter(|info| info.is_active).count() as u64;
        let num_active = num_active.max(1); // floor at 1
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.claim_rewards(current_slot, num_active)
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
        // AUDIT-FIX A-3: Use saturating_add to prevent overflow
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.delegated_amount = stake_info.delegated_amount.saturating_add(amount);
            // AUDIT-FIX A-1: Update is_active after delegation changes total stake
            stake_info.is_active = stake_info.meets_minimum();
        }

        // Delegations contribute to total active stake (used as denominator
        // in reward distribution and voting power calculations).
        self.total_staked = self.total_staked.saturating_add(amount);

        // Track individual delegation
        // AUDIT-FIX A-3: Use saturating_add to prevent overflow
        let validator_delegations = self.delegations.entry(*validator).or_default();
        let entry = validator_delegations.entry(delegator).or_insert(0);
        *entry = entry.saturating_add(amount);

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
            // AUDIT-FIX A-1: Update is_active after delegation changes total stake
            stake_info.is_active = stake_info.meets_minimum();
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
            self.total_staked = self.total_staked.saturating_add(additional);
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
        self.unstake_requests
            .iter()
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
        // A-6: Use only active validators' stake for average calculation
        let active_total: u64 = self
            .stakes
            .values()
            .filter(|s| s.is_active && s.meets_minimum())
            .map(|s| s.total_stake())
            .sum();
        let avg_stake = if active_validators > 0 {
            active_total / active_validators
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

    // ========================================================================
    // GRADUATION — Bootstrap grant management + machine fingerprint registry
    // ========================================================================

    /// Get the number of bootstrap grants issued so far.
    pub fn bootstrap_grants_issued(&self) -> u64 {
        self.bootstrap_grants_issued
    }

    /// Allocate the next bootstrap index. Returns `Some(index)` if under the cap,
    /// or `None` if the bootstrap phase is complete (≥ MAX_BOOTSTRAP_VALIDATORS).
    pub fn next_bootstrap_index(&mut self) -> Option<u64> {
        if self.bootstrap_grants_issued < MAX_BOOTSTRAP_VALIDATORS {
            let index = self.bootstrap_grants_issued;
            self.bootstrap_grants_issued += 1;
            Some(index)
        } else {
            None
        }
    }

    /// Register stake for a validator with explicit bootstrap index.
    /// Use this instead of `stake()` when the caller knows whether this is a
    /// bootstrap grant or a self-funded validator.
    pub fn stake_with_index(
        &mut self,
        validator: Pubkey,
        amount: u64,
        current_slot: u64,
        bootstrap_index: u64,
    ) -> Result<(), String> {
        if amount < MIN_VALIDATOR_STAKE {
            return Err(format!(
                "Stake {} is below minimum {}",
                amount, MIN_VALIDATOR_STAKE
            ));
        }

        // AUDIT-FIX A-2: Enforce MAX_VALIDATOR_STAKE cap
        if let Some(existing) = self.stakes.get(&validator) {
            let new_total = existing.amount.saturating_add(amount);
            if new_total > MAX_VALIDATOR_STAKE {
                return Err(format!(
                    "Stake {} would exceed max {} per validator",
                    new_total, MAX_VALIDATOR_STAKE
                ));
            }
        } else if amount > MAX_VALIDATOR_STAKE {
            return Err(format!(
                "Stake {} exceeds max {} per validator",
                amount, MAX_VALIDATOR_STAKE
            ));
        }

        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            stake_info.amount = stake_info.amount.saturating_add(amount);
            stake_info.is_active = stake_info.meets_minimum();
        } else {
            let stake_info =
                StakeInfo::with_bootstrap_index(validator, amount, current_slot, bootstrap_index);
            self.stakes.insert(validator, stake_info);
        }

        self.total_staked = self.total_staked.saturating_add(amount);
        Ok(())
    }

    /// Register a machine fingerprint for a validator.
    ///
    /// Returns `Ok(())` on success, or an error if the fingerprint is already
    /// claimed by a different validator (anti-Sybil).
    pub fn register_fingerprint(
        &mut self,
        validator: &Pubkey,
        fingerprint: [u8; 32],
    ) -> Result<(), String> {
        // All-zeros fingerprint = not set (dev mode or legacy), always accept
        if fingerprint == [0u8; 32] {
            return Ok(());
        }

        if let Some(existing) = self.fingerprint_registry.get(&fingerprint) {
            if existing != validator {
                return Err(format!(
                    "Machine fingerprint already registered to validator {}",
                    existing.to_base58()
                ));
            }
            // Same validator re-registering same fingerprint — idempotent
            return Ok(());
        }

        self.fingerprint_registry.insert(fingerprint, *validator);

        // Also store on the StakeInfo
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.machine_fingerprint = fingerprint;
        }

        Ok(())
    }

    /// Migrate a validator's fingerprint to a new machine.
    ///
    /// The old fingerprint is released after MIGRATION_COOLDOWN_SLOTS.
    /// Returns `Ok(())` on success, or an error if cooldown hasn't elapsed.
    pub fn migrate_fingerprint(
        &mut self,
        validator: &Pubkey,
        new_fingerprint: [u8; 32],
        current_slot: u64,
    ) -> Result<(), String> {
        if new_fingerprint == [0u8; 32] {
            return Err("Cannot migrate to empty fingerprint".to_string());
        }

        // Check new fingerprint isn't already taken
        if let Some(existing) = self.fingerprint_registry.get(&new_fingerprint) {
            if existing != validator {
                return Err(format!(
                    "New machine fingerprint already registered to validator {}",
                    existing.to_base58()
                ));
            }
        }

        let stake_info = self
            .stakes
            .get(validator)
            .ok_or_else(|| "Validator not found".to_string())?;

        // Enforce migration cooldown
        if stake_info.last_migration_slot > 0
            && current_slot < stake_info.last_migration_slot + MIGRATION_COOLDOWN_SLOTS
        {
            let remaining =
                (stake_info.last_migration_slot + MIGRATION_COOLDOWN_SLOTS) - current_slot;
            return Err(format!(
                "Migration cooldown active. {} slots remaining (~{} hours)",
                remaining,
                remaining * 400 / 1000 / 3600
            ));
        }

        // Remove old fingerprint mapping
        let old_fingerprint = stake_info.machine_fingerprint;
        if old_fingerprint != [0u8; 32] {
            self.fingerprint_registry.remove(&old_fingerprint);
        }

        // Register new fingerprint
        self.fingerprint_registry
            .insert(new_fingerprint, *validator);

        // Update StakeInfo
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            stake_info.machine_fingerprint = new_fingerprint;
            stake_info.last_migration_slot = current_slot;
        }

        Ok(())
    }

    /// Check if a fingerprint is already registered (and to which validator).
    pub fn fingerprint_owner(&self, fingerprint: &[u8; 32]) -> Option<&Pubkey> {
        self.fingerprint_registry.get(fingerprint)
    }

    /// Atomically: validate fingerprint → allocate bootstrap index → stake → register.
    ///
    /// This prevents the bug where `next_bootstrap_index()` increments the counter
    /// and then `register_fingerprint()` fails, wasting a bootstrap slot.
    ///
    /// Returns `Ok((bootstrap_index, is_new))` on success.
    /// `bootstrap_index` is `u64::MAX` if the bootstrap phase is complete.
    /// `is_new` is true if this was a new stake entry (not an existing restake).
    pub fn try_bootstrap_with_fingerprint(
        &mut self,
        validator: Pubkey,
        amount: u64,
        current_slot: u64,
        fingerprint: [u8; 32],
    ) -> Result<(u64, bool), String> {
        // If validator already exists, ensure idempotent bootstrap.
        // A validator that already has >= the requested amount is fully
        // bootstrapped — return early to prevent double-accounting.
        // If below the target, top up to the requested amount (not accumulate).
        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            if stake_info.amount >= amount {
                // Already at or above target — idempotent no-op
                let existing_index = stake_info.bootstrap_index;
                if fingerprint != [0u8; 32] {
                    self.register_fingerprint(&validator, fingerprint)?;
                }
                return Ok((existing_index, false));
            }
            // Below target: bring up to the requested amount
            let deficit = amount - stake_info.amount;
            // AUDIT-FIX A-2: Enforce MAX_VALIDATOR_STAKE cap
            if amount > MAX_VALIDATOR_STAKE {
                return Err(format!(
                    "Stake {} would exceed max {} per validator",
                    amount, MAX_VALIDATOR_STAKE
                ));
            }
            stake_info.amount = amount;
            stake_info.is_active = stake_info.meets_minimum();
            let existing_index = stake_info.bootstrap_index;
            self.total_staked = self.total_staked.saturating_add(deficit);
            // Register fingerprint (idempotent for same validator)
            if fingerprint != [0u8; 32] {
                self.register_fingerprint(&validator, fingerprint)?;
            }
            return Ok((existing_index, false));
        }

        // New validator — check fingerprint BEFORE allocating bootstrap index
        if fingerprint != [0u8; 32] {
            if let Some(existing) = self.fingerprint_registry.get(&fingerprint) {
                if existing != &validator {
                    return Err(format!(
                        "Machine fingerprint already registered to validator {}. \
                         Each machine can only receive one bootstrap grant.",
                        existing.to_base58()
                    ));
                }
            }
        }

        // Fingerprint is unique — safe to allocate bootstrap index
        let bootstrap_index = if self.bootstrap_grants_issued < MAX_BOOTSTRAP_VALIDATORS {
            let idx = self.bootstrap_grants_issued;
            self.bootstrap_grants_issued += 1;
            idx
        } else {
            u64::MAX // Self-funded
        };

        // Create stake entry
        if amount < MIN_VALIDATOR_STAKE {
            // Roll back counter if we allocated an index
            if bootstrap_index < MAX_BOOTSTRAP_VALIDATORS {
                self.bootstrap_grants_issued -= 1;
            }
            return Err(format!(
                "Stake {} is below minimum {}",
                amount, MIN_VALIDATOR_STAKE
            ));
        }

        let stake_info =
            StakeInfo::with_bootstrap_index(validator, amount, current_slot, bootstrap_index);
        self.stakes.insert(validator, stake_info);
        self.total_staked = self.total_staked.saturating_add(amount);

        // Register fingerprint
        if fingerprint != [0u8; 32] {
            self.fingerprint_registry.insert(fingerprint, validator);
            if let Some(si) = self.stakes.get_mut(&validator) {
                si.machine_fingerprint = fingerprint;
            }
        }

        Ok((bootstrap_index, true))
    }

    /// Get mutable reference to stake info (for direct field updates in validator)
    pub fn get_stake_mut(&mut self, validator: &Pubkey) -> Option<&mut StakeInfo> {
        self.stakes.get_mut(validator)
    }

    /// One-time migration: re-assign bootstrap indices to validators that were
    /// staked before the contributory-stake system existed. These validators have
    /// `bootstrap_index == u64::MAX` (the default from `StakeInfo::new()`) even
    /// though they joined within the first 200 and deserve a bootstrap grant.
    ///
    /// Returns the number of validators migrated.
    pub fn migrate_legacy_bootstrap_indices(&mut self) -> u64 {
        // Collect validators that need migration: index==MAX, amount==MIN_VALIDATOR_STAKE,
        // and status is FullyVested (incorrectly — they were never vested, just default)
        let mut to_migrate: Vec<Pubkey> = self
            .stakes
            .iter()
            .filter(|(_, info)| {
                info.bootstrap_index == u64::MAX
                    && info.amount >= MIN_VALIDATOR_STAKE
                    && info.bootstrap_debt == 0
                    && info.graduation_slot.is_none()
                    && info.total_debt_repaid == 0
            })
            .map(|(pubkey, _)| *pubkey)
            .collect();

        // Sort by start_slot for deterministic ordering
        to_migrate.sort_by_key(|pk| {
            self.stakes
                .get(pk)
                .map(|s| s.start_slot)
                .unwrap_or(u64::MAX)
        });

        let mut migrated = 0u64;
        for pubkey in to_migrate {
            if self.bootstrap_grants_issued >= MAX_BOOTSTRAP_VALIDATORS {
                break; // No more bootstrap slots available
            }
            let idx = self.bootstrap_grants_issued;
            self.bootstrap_grants_issued += 1;

            if let Some(stake_info) = self.stakes.get_mut(&pubkey) {
                stake_info.bootstrap_index = idx;
                stake_info.bootstrap_debt = stake_info.amount; // Full debt = 100K MOLT
                stake_info.status = BootstrapStatus::Bootstrapping;
                migrated += 1;
            }
        }
        migrated
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

// ============================================================================
// VOTE AUTHORITY — Single Vote Gatekeeper (like Eth2 Slashing Protection DB)
// ============================================================================

/// Single Vote Authority — prevents double-voting at the signing level.
///
/// This struct is the **sole gatekeeper** for vote creation. It atomically
/// checks whether we've already signed a vote for a slot, and only signs
/// a new vote if no prior vote exists for that slot. This design mirrors
/// Ethereum 2.0's slashing-protection database:
/// - One authority per validator, shared across all tasks via `Arc<Mutex<>>`.
/// - The signing key lives ONLY inside VoteAuthority — no other code path
///   can create a valid signed vote.
/// - Prevents all three DoubleVote scenarios: P2P echo, fork re-evaluation,
///   and view rotation races.
pub struct VoteAuthority {
    /// Signing seed — used to reconstruct Ed25519 keypair for each signing op.
    signing_seed: [u8; 32],
    /// Our validator public key.
    validator_pubkey: Pubkey,
    /// Map of slot → block_hash for slots we've already voted on.
    /// This is the slot-level lock: once a slot is recorded, no second vote
    /// for a DIFFERENT hash can be produced.
    voted: std::collections::HashMap<u64, Hash>,
}

impl VoteAuthority {
    /// Create a new VoteAuthority that owns the signing key.
    pub fn new(signing_seed: [u8; 32], validator_pubkey: Pubkey) -> Self {
        Self {
            signing_seed,
            validator_pubkey,
            voted: std::collections::HashMap::new(),
        }
    }

    /// Attempt to create a signed vote for the given slot and block hash.
    ///
    /// Returns:
    /// - `Some(vote)` if this is the first vote for this slot → signs and records.
    /// - `None` if we already voted for this slot (same OR different hash) → refuses to sign.
    ///
    /// When the existing vote matches the same hash, this is a benign P2P echo.
    /// When the existing vote has a DIFFERENT hash, this would be equivocation
    /// (DoubleVote) — the authority refuses to sign, preventing slashing.
    pub fn try_vote(&mut self, slot: u64, block_hash: Hash) -> Option<Vote> {
        if let Some(existing_hash) = self.voted.get(&slot) {
            if *existing_hash == block_hash {
                // Benign: already voted for exact same (slot, hash). P2P echo.
                tracing::debug!(
                    "VoteAuthority: slot {} already voted (same hash) — skipping",
                    slot
                );
            } else {
                // DANGER: Different hash at same slot → would be equivocation!
                tracing::warn!(
                    "🚨 VoteAuthority REFUSED equivocating vote: slot {} already voted \
                     hash {}, rejecting hash {}",
                    slot,
                    hex::encode(&existing_hash.0[..4]),
                    hex::encode(&block_hash.0[..4]),
                );
            }
            return None;
        }

        // First vote for this slot — sign and record atomically.
        let keypair = crate::Keypair::from_seed(&self.signing_seed);
        let mut vote_message = Vec::new();
        vote_message.extend_from_slice(&slot.to_le_bytes());
        vote_message.extend_from_slice(&block_hash.0);
        let signature = keypair.sign(&vote_message);

        let vote = Vote::new(slot, block_hash, self.validator_pubkey, signature);
        self.voted.insert(slot, block_hash);
        Some(vote)
    }

    /// Check if we've already voted for a given slot.
    pub fn has_voted(&self, slot: u64) -> bool {
        self.voted.contains_key(&slot)
    }

    /// Prune voted entries older than `min_slot` to bound memory.
    pub fn prune(&mut self, min_slot: u64) {
        self.voted.retain(|&s, _| s >= min_slot);
    }

    /// Number of tracked voted slots (for diagnostics).
    pub fn voted_count(&self) -> usize {
        self.voted.len()
    }
}

// ============================================================================
// BFT CONSENSUS — Tendermint-style Propose/Prevote/Precommit
// ============================================================================

/// BFT consensus round step.
///
/// Each height progresses through Propose → Prevote → Precommit → Commit.
/// If consensus fails in a round, the engine advances to the next round
/// with a new proposer (back to Propose).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundStep {
    /// Waiting for the designated proposer to broadcast a block.
    Propose,
    /// Waiting for 2/3+ stake-weighted prevotes.
    Prevote,
    /// Waiting for 2/3+ stake-weighted precommits.
    Precommit,
    /// Block committed — advancing to next height.
    Commit,
}

/// BFT Proposal — proposer broadcasts a block for validators to vote on.
///
/// Contains the full block so validators can verify it before prevoting.
/// The `valid_round` field enables Tendermint's proof-of-lock (POL) change:
/// a proposer may re-propose a block that received 2/3+ prevotes in a
/// prior round, allowing locked validators to unlock safely.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Block height (slot number) being proposed.
    pub height: u64,
    /// Round within this height (0-indexed).
    pub round: u32,
    /// The proposed block.
    pub block: Block,
    /// Round in which this block previously received 2/3+ prevotes,
    /// or -1 if this is a fresh proposal with no prior POL.
    pub valid_round: i32,
    /// Public key of the proposer.
    pub proposer: Pubkey,
    /// Ed25519 signature over (height || round || block_hash || valid_round).
    #[serde(
        serialize_with = "serialize_signature",
        deserialize_with = "deserialize_signature"
    )]
    pub signature: [u8; 64],
}

impl Proposal {
    /// Construct the message bytes for signing/verification.
    pub fn signable_bytes(&self) -> Vec<u8> {
        let block_hash = self.block.hash();
        let mut msg = Vec::with_capacity(48);
        msg.extend_from_slice(&self.height.to_le_bytes());
        msg.extend_from_slice(&self.round.to_le_bytes());
        msg.extend_from_slice(&block_hash.0);
        msg.extend_from_slice(&self.valid_round.to_le_bytes());
        msg
    }

    /// Verify the proposer's Ed25519 signature.
    pub fn verify_signature(&self) -> bool {
        let msg = self.signable_bytes();
        crate::Keypair::verify(&self.proposer, &msg, &self.signature)
    }

    /// Static helper to compute proposal signable bytes from components,
    /// without needing a full Proposal instance.
    pub fn signable_bytes_static(
        height: u64,
        round: u32,
        block_hash: &Hash,
        valid_round: i32,
    ) -> Vec<u8> {
        let mut msg = Vec::with_capacity(48);
        msg.extend_from_slice(&height.to_le_bytes());
        msg.extend_from_slice(&round.to_le_bytes());
        msg.extend_from_slice(&block_hash.0);
        msg.extend_from_slice(&valid_round.to_le_bytes());
        msg
    }
}

/// BFT Prevote — a validator's first-round attestation for a block or nil.
///
/// `block_hash = None` is a nil prevote, indicating the validator did not
/// receive a valid proposal in time (or the proposal was invalid).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prevote {
    /// Block height (slot number).
    pub height: u64,
    /// Round within this height.
    pub round: u32,
    /// Hash of the block being prevoted, or `None` for a nil prevote.
    pub block_hash: Option<Hash>,
    /// Validator who cast this prevote.
    pub validator: Pubkey,
    /// Ed25519 signature over (height || round || block_hash_or_nil).
    #[serde(
        serialize_with = "serialize_signature",
        deserialize_with = "deserialize_signature"
    )]
    pub signature: [u8; 64],
}

impl Prevote {
    /// Construct the message bytes for signing/verification.
    pub fn signable_bytes(height: u64, round: u32, block_hash: &Option<Hash>) -> Vec<u8> {
        let mut msg = Vec::with_capacity(48);
        msg.push(0x01); // prevote tag
        msg.extend_from_slice(&height.to_le_bytes());
        msg.extend_from_slice(&round.to_le_bytes());
        match block_hash {
            Some(h) => msg.extend_from_slice(&h.0),
            None => msg.extend_from_slice(&[0u8; 32]),
        }
        msg
    }

    /// Verify the voter's Ed25519 signature.
    pub fn verify_signature(&self) -> bool {
        let msg = Self::signable_bytes(self.height, self.round, &self.block_hash);
        crate::Keypair::verify(&self.validator, &msg, &self.signature)
    }
}

/// BFT Precommit — a validator's second-round attestation for a block or nil.
///
/// 2/3+ stake-weighted precommits for the same `block_hash` triggers
/// block commitment. 2/3+ nil precommits (or timeout) advances to the
/// next round.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Precommit {
    /// Block height (slot number).
    pub height: u64,
    /// Round within this height.
    pub round: u32,
    /// Hash of the block being precommitted, or `None` for a nil precommit.
    pub block_hash: Option<Hash>,
    /// Validator who cast this precommit.
    pub validator: Pubkey,
    /// Ed25519 signature over (height || round || block_hash_or_nil).
    #[serde(
        serialize_with = "serialize_signature",
        deserialize_with = "deserialize_signature"
    )]
    pub signature: [u8; 64],
}

impl Precommit {
    /// Construct the message bytes for signing/verification.
    pub fn signable_bytes(height: u64, round: u32, block_hash: &Option<Hash>) -> Vec<u8> {
        let mut msg = Vec::with_capacity(48);
        msg.push(0x02); // precommit tag
        msg.extend_from_slice(&height.to_le_bytes());
        msg.extend_from_slice(&round.to_le_bytes());
        match block_hash {
            Some(h) => msg.extend_from_slice(&h.0),
            None => msg.extend_from_slice(&[0u8; 32]),
        }
        msg
    }

    /// Verify the voter's Ed25519 signature.
    pub fn verify_signature(&self) -> bool {
        let msg = Self::signable_bytes(self.height, self.round, &self.block_hash);
        crate::Keypair::verify(&self.validator, &msg, &self.signature)
    }
}

/// Validator information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    /// Validator's public key
    pub pubkey: Pubkey,
    /// Reputation score (50-1000)
    /// Unified rules: init=100, +10/block (cap 1000), slashing subtracts severity (floor 50)
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
    /// Commission rate in basis points (0-10000). Default = 500 (5%).
    /// Backward-compatible: existing serialized data without this field defaults to 500.
    #[serde(default = "default_commission_rate")]
    pub commission_rate: u64,
    /// Total transactions processed (included in blocks produced by this validator)
    #[serde(default)]
    pub transactions_processed: u64,
}

fn default_commission_rate() -> u64 {
    500
}

impl ValidatorInfo {
    /// Create new validator
    pub fn new(pubkey: Pubkey, joined_slot: u64) -> Self {
        ValidatorInfo {
            pubkey,
            reputation: 100, // Unified: all validators start at 100
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 0,
            joined_slot,
            last_active_slot: joined_slot,
            commission_rate: 500, // 5% default (basis points)
            transactions_processed: 0,
        }
    }

    /// Update reputation based on performance
    /// Unified rules: +10 per block (capped at 1000), -50 penalty (floor 50)
    pub fn update_reputation(&mut self, correct: bool) {
        if correct {
            // Increase reputation (max 1000)
            self.reputation = (self.reputation + 10).min(1000);
        } else {
            // Decrease reputation (floor 50 — prevents "Newcomer" death spiral)
            self.reputation = self.reputation.saturating_sub(50).max(50);
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
    /// Reputation influence uses sqrt() to prevent snowball: rep 1000 gets ~3.16x weight
    /// vs rep 100 (instead of 10x with linear).  Ensures fairer block distribution.
    ///
    /// A5-01: Mixes optional `randomness_seed` (previous block hash) into the
    /// selection hash to prevent predictability beyond one slot ahead. Without
    /// a seed, falls back to SHA-256(slot) for backward compatibility.
    pub fn select_leader_weighted(&self, slot: u64, stake_pool: &StakePool) -> Option<Pubkey> {
        self.select_leader_weighted_with_seed(slot, stake_pool, &[])
    }

    /// Select leader with explicit randomness seed (e.g., previous block hash).
    /// This is the primary entry point for production validators.
    ///
    /// Uses **Tendermint-style weighted round-robin** (CometBFT proposer
    /// selection) to guarantee fair, proportional leader rotation:
    ///   1. Compute each validator's weight from sqrt(stake).
    ///   2. Simulate `slot` rounds of the algorithm starting from a
    ///      deterministic initial state derived from `randomness_seed`.
    ///   3. Each round: add weight to every priority → pick highest → subtract
    ///      total weight from the winner.
    ///
    /// Over N rounds every validator leads ~(weight/total)*N times, eliminating
    /// the clustering problem of pure hash-based random selection.
    ///
    /// Leader election depends ONLY on stake (immutable within epoch) so all
    /// validators deterministically agree on who the leader is for any given
    /// (slot, seed) pair. Reputation has no influence on leader selection.
    ///
    /// Validators below MIN_VALIDATOR_STAKE are EXCLUDED from leader rotation.
    /// This prevents 0-stake (slashed) validators from producing blocks.
    pub fn select_leader_weighted_with_seed(
        &self,
        slot: u64,
        stake_pool: &StakePool,
        randomness_seed: &[u8],
    ) -> Option<Pubkey> {
        if self.validators.is_empty() {
            return None;
        }

        let sorted_validators = self.sorted_validators();

        // Filter: only validators with stake >= MIN_VALIDATOR_STAKE can be leaders
        let eligible: Vec<&ValidatorInfo> = sorted_validators
            .iter()
            .filter(|v| {
                let stake = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake())
                    .unwrap_or(v.stake);
                stake >= MIN_VALIDATOR_STAKE
            })
            .collect();

        // MIN-STAKE GUARD: If no validators meet minimum stake, the chain
        // halts rather than producing blocks with 0-stake validators.
        if eligible.is_empty() {
            eprintln!("🛑 HALT: No validators meet MIN_VALIDATOR_STAKE — chain cannot produce blocks safely");
            return None;
        }

        let n = eligible.len();
        if n == 1 {
            return Some(eligible[0].pubkey);
        }

        // ── Compute weights: stake-only (no reputation) ──
        // Uses sqrt(stake) + 1 for proportional-but-not-dominant leader rotation.
        // Reputation is excluded from leader election: leader selection must
        // depend ONLY on immutable-within-epoch state so all validators agree.
        let weights: Vec<i128> = eligible
            .iter()
            .map(|v| {
                let stake = stake_pool
                    .get_stake(&v.pubkey)
                    .map(|s| s.total_stake().min(MAX_VALIDATOR_STAKE))
                    .unwrap_or_else(|| v.stake.min(MAX_VALIDATOR_STAKE));

                (integer_sqrt(stake.max(1)) as i128) + 1
            })
            .collect();

        let total_weight: i128 = weights.iter().sum();
        if total_weight == 0 {
            return self.select_leader(slot);
        }

        // ── Tendermint weighted round-robin ──
        // Deterministic initial priorities seeded by `randomness_seed` so that
        // after a view change (where `slot` is remapped) a different validator
        // may win, preventing permanent stalls when one leader is offline.
        //
        // The algorithm is O(n) per simulated round.  We only need to simulate
        // (slot % (n * EPOCH_LEN)) rounds — the algorithm is periodic with
        // period = LCM of weights (bounded by n * max_weight).  For small
        // validator sets (≤ 500) this is fast.  For larger sets the epoch
        // shortcut keeps it bounded.
        let epoch_len = (n as u64) * 4; // 4 full rotations per epoch
        let effective_rounds = (slot % epoch_len.max(1)) as usize;

        // Seed initial priorities with a small deterministic offset derived
        // from the randomness_seed.  This ensures that after a parent-hash
        // change, the ordering is shuffled while still being deterministic
        // across all validators who share the same chain tip.
        let mut priorities: Vec<i128> = if randomness_seed.is_empty() {
            vec![0i128; n]
        } else {
            eligible
                .iter()
                .enumerate()
                .map(|(i, _v)| {
                    let mut pre = Vec::with_capacity(randomness_seed.len() + 8);
                    pre.extend_from_slice(randomness_seed);
                    pre.extend_from_slice(&(i as u64).to_le_bytes());
                    let h = Hash::hash(&pre);
                    let mut sb = [0u8; 8];
                    sb.copy_from_slice(&h.0[..8]);
                    // Small offset in [-total_weight/2, +total_weight/2]
                    let raw = i64::from_le_bytes(sb) as i128;
                    raw % (total_weight / 2 + 1)
                })
                .collect()
        };

        // Simulate `effective_rounds + 1` rounds of the algorithm (round 0 = first slot)
        let mut proposer_idx = 0;
        for _ in 0..=effective_rounds {
            // Add weight to each validator's priority
            for (i, w) in weights.iter().enumerate() {
                priorities[i] += *w;
            }
            // Pick the validator with the highest priority (tie-break by index = deterministic)
            proposer_idx = 0;
            let mut max_priority = priorities[0];
            for (i, &p) in priorities.iter().enumerate().take(n).skip(1) {
                if p > max_priority {
                    max_priority = p;
                    proposer_idx = i;
                }
            }
            // Decrease proposer's priority by total weight
            priorities[proposer_idx] -= total_weight;
        }

        Some(eligible[proposer_idx].pubkey)
    }
}

// AUDIT-FIX 3.3: Use Newton's method with pure integer arithmetic
// instead of f64 intermediate for deterministic consensus results.
fn integer_sqrt(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
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
    /// PERF-OPT 5: Secondary index for O(1) equivocation detection.
    /// Maps (slot, validator) → true. Previously the equivocation check
    /// scanned ALL votes across ALL slots = O(total_votes). With 500
    /// validators and 100s of retained slots, this was ~50k iterations
    /// per add_vote call. Now it's a single HashMap lookup.
    voted_in_slot: std::collections::HashMap<(u64, Pubkey), bool>,
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
            voted_in_slot: std::collections::HashMap::new(),
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
        // PERF-OPT 5: O(1) lookup via secondary index instead of full scan.
        if self
            .voted_in_slot
            .contains_key(&(vote.slot, vote.validator))
        {
            return false; // equivocation attempt
        }

        self.voted_in_slot.insert((vote.slot, vote.validator), true);
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
        // PERF-OPT 5: Keep secondary equivocation index in sync with pruning
        self.voted_in_slot
            .retain(|(slot, _), _| *slot >= cutoff_slot);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// FINALITY TRACKER — Lock-free commitment level tracking
// ═══════════════════════════════════════════════════════════════════════════════

/// Number of confirmed slots before a block is considered finalized.
/// Matches Solana's 32-slot finality depth.
pub const FINALITY_DEPTH: u64 = 32;

/// Lock-free finality tracker shared between validator consensus and RPC.
///
/// Tracks three commitment levels (Solana-compatible):
///   - **Processed**: Block stored on chain tip (existing behavior via `last_slot`)
///   - **Confirmed**: Block has received 2/3 stake-weighted supermajority votes
///   - **Finalized**: Confirmed + FINALITY_DEPTH slots deep (safe from rollback)
///
/// Uses `AtomicU64` for zero-cost reads from RPC without locking the vote aggregator.
#[derive(Debug, Clone)]
pub struct FinalityTracker {
    /// Highest slot that has reached 2/3 supermajority
    confirmed_slot: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Highest slot considered finalized (confirmed_slot - FINALITY_DEPTH)
    finalized_slot: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl FinalityTracker {
    /// Create a new finality tracker, optionally loading persisted values
    pub fn new(initial_confirmed: u64, initial_finalized: u64) -> Self {
        use std::sync::atomic::AtomicU64;
        use std::sync::Arc;
        FinalityTracker {
            confirmed_slot: Arc::new(AtomicU64::new(initial_confirmed)),
            finalized_slot: Arc::new(AtomicU64::new(initial_finalized)),
        }
    }

    /// Called when a block reaches supermajority. Updates confirmed and finalized slots.
    /// Returns true if the confirmed slot was actually advanced.
    pub fn mark_confirmed(&self, slot: u64) -> bool {
        use std::sync::atomic::Ordering;
        let prev = self.confirmed_slot.fetch_max(slot, Ordering::Relaxed);
        if slot > prev {
            // Advance finalized slot: any confirmed slot >= FINALITY_DEPTH behind tip
            let new_finalized = slot.saturating_sub(FINALITY_DEPTH);
            self.finalized_slot
                .fetch_max(new_finalized, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Get the current confirmed slot (2/3 supermajority reached)
    pub fn confirmed_slot(&self) -> u64 {
        self.confirmed_slot
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Get the current finalized slot (confirmed + FINALITY_DEPTH deep)
    pub fn finalized_slot(&self) -> u64 {
        self.finalized_slot
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Determine the commitment level of a transaction in a given slot.
    ///   - `None` if the slot hasn't been processed
    ///   - `"processed"` if the tx is in a block but not yet confirmed
    ///   - `"confirmed"` if the block has 2/3 votes but isn't finalized yet
    ///   - `"finalized"` if the block is confirmed + 32 slots deep
    pub fn commitment_for_slot(&self, slot: u64) -> &'static str {
        let finalized = self.finalized_slot();
        let confirmed = self.confirmed_slot();
        if slot <= finalized {
            "finalized"
        } else if slot <= confirmed {
            "confirmed"
        } else {
            "processed"
        }
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

    /// Select the canonical chain head, rejecting any head at or below the
    /// finalized slot.  This enforces the finality invariant: once a block is
    /// finalized it can never be reverted, so a competing head that would
    /// require reverting past finality is simply ignored.
    pub fn select_head_respecting_finality(&self, finalized_slot: u64) -> Option<(u64, Hash)> {
        self.heads
            .iter()
            .filter(|(slot, _, _)| *slot > finalized_slot)
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
    /// P9-CORE-02: Accept deterministic block_timestamp instead of using SystemTime::now()
    pub fn new(
        offense: SlashingOffense,
        validator: Pubkey,
        evidence_slot: u64,
        reporter: Pubkey,
        block_timestamp: u64,
    ) -> Self {
        SlashingEvidence {
            offense,
            validator,
            evidence_slot,
            reporter,
            timestamp: block_timestamp,
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
///
/// ## Tiered Downtime Penalty System
///
/// Downtime offenses use a 3-tier escalation:
/// - **Tier 1** (1st offense): Reputation penalty only — warning, no economic slash
/// - **Tier 2** (2nd offense): Small slash (0.5% of stake) + temporary suspension (100 slots)
///   + increased bootstrap debt repayment rate (90% of rewards go to debt)
/// - **Tier 3** (3rd+ offense): Full downtime slashing per ConsensusParams
///   (1% per 100 missed slots, max 10%)
///
/// Offense count decays (forgiveness) after 50,000 slots (~5.5 hours) of no new offenses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingTracker {
    /// Evidence by validator
    evidence: std::collections::HashMap<Pubkey, Vec<SlashingEvidence>>,
    /// Slashed validators
    slashed: std::collections::HashSet<Pubkey>,
    /// Permanently banned validators (collusion = permanent ban per whitepaper)
    permanently_banned: std::collections::HashSet<Pubkey>,
    /// Tiered downtime offense count per validator (for escalation)
    #[serde(default)]
    downtime_offense_count: std::collections::HashMap<Pubkey, u64>,
    /// Last downtime offense slot per validator (for forgiveness decay)
    #[serde(default)]
    last_downtime_offense_slot: std::collections::HashMap<Pubkey, u64>,
    /// Validators with penalty repayment boost active (slot when boost expires)
    #[serde(default)]
    penalty_repayment_boost: std::collections::HashMap<Pubkey, u64>,
    /// Validators temporarily suspended (slot when suspension ends)
    #[serde(default)]
    suspended_until: std::collections::HashMap<Pubkey, u64>,
    /// AUDIT-FIX HIGH-4: Track how many downtime evidence entries we last processed
    /// per validator. Only record a new offense when new evidence appears, preventing
    /// the sweep from escalating tiers every 100 slots on the same evidence.
    #[serde(default)]
    last_processed_downtime_evidence_count: std::collections::HashMap<Pubkey, usize>,
}

/// Number of slots of good behavior before downtime offense count decays to zero
pub const DOWNTIME_FORGIVENESS_SLOTS: u64 = 50_000; // ~5.5 hours at 400ms/slot

/// Suspension duration in slots for Tier 2 downtime offense
pub const DOWNTIME_SUSPENSION_SLOTS: u64 = 100; // ~40 seconds

/// Tier 2 small slash percentage (basis points — 50 = 0.5%)
pub const DOWNTIME_TIER2_SLASH_BPS: u64 = 50;

/// Duration of penalty repayment boost in slots after Tier 2 offense
pub const PENALTY_REPAYMENT_BOOST_SLOTS: u64 = 216_000; // ~1 day

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
            downtime_offense_count: std::collections::HashMap::new(),
            last_downtime_offense_slot: std::collections::HashMap::new(),
            penalty_repayment_boost: std::collections::HashMap::new(),
            suspended_until: std::collections::HashMap::new(),
            last_processed_downtime_evidence_count: std::collections::HashMap::new(),
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
                // AUDIT-FIX LOW-12: Deduplicate Downtime evidence by last_active_slot
                // instead of missed_slots. Detection loop runs periodically and
                // may produce entries with growing missed_slots (100→200→300) for
                // the SAME downtime event. The last_active_slot is stable — it marks
                // when the validator went offline — so it correctly deduplicates.
                (
                    SlashingOffense::Downtime {
                        last_active_slot: la1,
                        ..
                    },
                    SlashingOffense::Downtime {
                        last_active_slot: la2,
                        ..
                    },
                ) => la1 == la2,
                _ => false,
            })
        {
            return false; // Already have evidence for this offense
        }

        validator_evidence.push(evidence);
        true
    }

    /// Total number of evidence records across all validators
    pub fn evidence_count(&self) -> usize {
        self.evidence.values().map(|v| v.len()).sum()
    }

    /// Check if validator should be slashed.
    ///
    /// For non-downtime offenses: severity >= 70 (DoubleBlock, DoubleVote,
    /// InvalidStateTransition, Censorship, Collusion).
    ///
    /// For downtime offenses: uses tiered system based on offense count:
    /// - Tier 1 (1st offense): NO economic slash (reputation only)
    /// - Tier 2 (2nd offense): Small slash (0.5%)
    /// - Tier 3 (3rd+ offense): Full graduated slashing
    pub fn should_slash(&self, validator: &Pubkey, current_slot: u64) -> bool {
        if let Some(evidence_list) = self.evidence.get(validator) {
            // Always slash for severe non-downtime offenses (severity >= 70)
            let has_severe = evidence_list.iter().any(|e| {
                e.severity() >= 70 && !matches!(e.offense, SlashingOffense::Downtime { .. })
            });
            if has_severe {
                return true;
            }

            // For downtime-only evidence, check the tiered system
            let has_downtime = evidence_list
                .iter()
                .any(|e| matches!(e.offense, SlashingOffense::Downtime { .. }));
            if has_downtime {
                let offense_count = self.get_downtime_offense_tier(validator, current_slot);
                // Tier 2+ gets economic slashing
                return offense_count >= 2;
            }

            false
        } else {
            false
        }
    }

    /// Get the current downtime offense tier for a validator, applying forgiveness
    /// decay check without mutating state.
    /// AUDIT-FIX HIGH-5: Now takes current_slot and checks forgiveness window.
    /// Previously returned stale count — if forgiveness expired, the validator
    /// would still be treated as Tier 2/3 until record_downtime_offense ran.
    /// Returns the effective offense count (0 = no offenses, 1 = tier 1, 2 = tier 2, 3+ = tier 3).
    pub fn get_downtime_offense_tier(&self, validator: &Pubkey, current_slot: u64) -> u64 {
        let raw_count = self
            .downtime_offense_count
            .get(validator)
            .copied()
            .unwrap_or(0);
        if raw_count == 0 {
            return 0;
        }
        // Check if forgiveness has expired — if so, effective count is 0
        if let Some(&last_slot) = self.last_downtime_offense_slot.get(validator) {
            if current_slot.saturating_sub(last_slot) >= DOWNTIME_FORGIVENESS_SLOTS {
                return 0; // Forgiven
            }
        }
        raw_count
    }

    /// Record a new downtime offense for a validator, applying forgiveness decay first.
    /// Returns the new offense count (tier level).
    pub fn record_downtime_offense(&mut self, validator: &Pubkey, current_slot: u64) -> u64 {
        // Apply forgiveness decay: if enough slots have passed since last offense,
        // reset the count to zero before incrementing.
        if let Some(&last_slot) = self.last_downtime_offense_slot.get(validator) {
            if current_slot.saturating_sub(last_slot) >= DOWNTIME_FORGIVENESS_SLOTS {
                // Forgiveness: reset offense count
                self.downtime_offense_count.insert(*validator, 0);
            }
        }

        let count = self.downtime_offense_count.entry(*validator).or_insert(0);
        *count += 1;
        self.last_downtime_offense_slot
            .insert(*validator, current_slot);
        *count
    }

    /// AUDIT-FIX HIGH-4: Check if new downtime evidence has been added since
    /// we last processed this validator. Returns true if there is fresh evidence
    /// that hasn't been counted yet, and updates the tracked count.
    /// This prevents the sweep from calling record_downtime_offense every 100 slots
    /// on the same evidence, which would escalate Tier 1→2 in 40 seconds.
    pub fn has_new_downtime_evidence(&mut self, validator: &Pubkey) -> bool {
        let current_downtime_count = self
            .evidence
            .get(validator)
            .map(|ev| {
                ev.iter()
                    .filter(|e| matches!(e.offense, SlashingOffense::Downtime { .. }))
                    .count()
            })
            .unwrap_or(0);

        let last_count = self
            .last_processed_downtime_evidence_count
            .get(validator)
            .copied()
            .unwrap_or(0);

        if current_downtime_count > last_count {
            self.last_processed_downtime_evidence_count
                .insert(*validator, current_downtime_count);
            true
        } else {
            false
        }
    }

    /// Check if a validator is currently suspended (Tier 2 temporary suspension)
    pub fn is_suspended(&self, validator: &Pubkey, current_slot: u64) -> bool {
        if let Some(&suspend_end) = self.suspended_until.get(validator) {
            current_slot < suspend_end
        } else {
            false
        }
    }

    /// Apply temporary suspension to a validator (Tier 2 penalty)
    pub fn suspend_validator(&mut self, validator: &Pubkey, current_slot: u64) {
        self.suspended_until
            .insert(*validator, current_slot + DOWNTIME_SUSPENSION_SLOTS);
    }

    /// Check if a validator has a penalty repayment boost active
    pub fn has_penalty_repayment_boost(&self, validator: &Pubkey, current_slot: u64) -> bool {
        if let Some(&boost_end) = self.penalty_repayment_boost.get(validator) {
            current_slot < boost_end
        } else {
            false
        }
    }

    /// Apply penalty repayment boost (Tier 2 — 90% of rewards go to bootstrap debt)
    pub fn apply_penalty_repayment_boost(&mut self, validator: &Pubkey, current_slot: u64) {
        self.penalty_repayment_boost
            .insert(*validator, current_slot + PENALTY_REPAYMENT_BOOST_SLOTS);
    }

    /// Clear the slashed flag for a validator (allows recovery after top-up).
    /// Does NOT clear permanent bans.
    pub fn clear_slashed(&mut self, validator: &Pubkey) {
        if !self.is_permanently_banned(validator) {
            self.slashed.remove(validator);
        }
    }

    /// Clean up expired suspensions and repayment boosts
    pub fn cleanup_expired(&mut self, current_slot: u64) {
        self.suspended_until
            .retain(|_, &mut end| current_slot < end);
        self.penalty_repayment_boost
            .retain(|_, &mut end| current_slot < end);
    }

    /// Mark validator as slashed
    pub fn slash(&mut self, validator: &Pubkey, current_slot: u64) -> bool {
        if self.should_slash(validator, current_slot) {
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

    /// Iterate over all validators currently marked as slashed.
    /// Used by the sweep to clear slashed flags after processing.
    pub fn slashed_validators(&self) -> impl Iterator<Item = Pubkey> + '_ {
        self.slashed.iter().copied()
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
    /// AUDIT-FIX A5-03: Now reads slashing percentages from ConsensusParams
    /// instead of hardcoding them, ensuring genesis config is the single source
    /// of truth for all slashing parameters.
    pub fn apply_economic_slashing(
        &mut self,
        validator: &Pubkey,
        stake_pool: &mut StakePool,
    ) -> u64 {
        // Default params for backward compatibility (tests that don't pass config)
        let default_params = ConsensusParams::default();
        self.apply_economic_slashing_with_params(validator, stake_pool, &default_params, 0)
    }

    /// Apply economic slashing using explicit consensus parameters.
    ///
    /// For downtime offenses, applies tiered penalties:
    /// - Tier 1 (1st offense): No economic slash (reputation only)
    /// - Tier 2 (2nd offense): 0.5% of stake + suspension + repayment boost
    /// - Tier 3 (3rd+ offense): Full graduated slashing per ConsensusParams
    pub fn apply_economic_slashing_with_params(
        &mut self,
        validator: &Pubkey,
        stake_pool: &mut StakePool,
        params: &ConsensusParams,
        current_slot: u64,
    ) -> u64 {
        // AUDIT-FIX CP-9: Snapshot stake BEFORE the loop to prevent compound slashing.
        let original_stake = stake_pool
            .get_stake(validator)
            .map(|s| s.total_stake())
            .unwrap_or(0);

        if original_stake == 0 {
            return 0;
        }

        let mut total_penalty = 0u64;
        let offense_tier = self.get_downtime_offense_tier(validator, current_slot);

        // Check if there are any non-downtime offenses that meet the severity threshold
        let has_non_downtime_slash = if let Some(evidence_list) = self.evidence.get(validator) {
            evidence_list.iter().any(|e| {
                e.severity() >= 70 && !matches!(e.offense, SlashingOffense::Downtime { .. })
            })
        } else {
            false
        };

        // For downtime-only cases, check if tier warrants slashing
        let has_downtime_slash = offense_tier >= 2;

        if !has_non_downtime_slash && !has_downtime_slash {
            return 0;
        }

        if let Some(evidence_list) = self.evidence.get(validator) {
            // AUDIT-FIX CRITICAL-3: Process downtime and non-downtime separately.
            // For downtime, use ONLY the worst entry (highest missed_slots) to
            // prevent multi-evidence inflation where N detection cycles each
            // contribute 0.5%, inflating the penalty far beyond the intended 0.5%.

            // 1. Find the worst downtime entry (highest missed_slots)
            let worst_downtime_missed = evidence_list
                .iter()
                .filter_map(|e| {
                    if let SlashingOffense::Downtime { missed_slots, .. } = e.offense {
                        Some(missed_slots)
                    } else {
                        None
                    }
                })
                .max();

            // 2. Apply downtime penalty once using worst entry
            if let Some(missed_slots) = worst_downtime_missed {
                let downtime_penalty = match offense_tier {
                    0 | 1 => 0, // Tier 1: No economic slash
                    2 => {
                        // Tier 2: 0.5% of stake (applied ONCE regardless of evidence count)
                        (original_stake as u128 * DOWNTIME_TIER2_SLASH_BPS as u128 / 10_000) as u64
                    }
                    _ => {
                        // Tier 3+: Full graduated using worst entry's missed_slots
                        let dp = (missed_slots / 100).min(params.slashing_downtime_max_percent);
                        (original_stake as u128
                            * dp as u128
                            * params.slashing_downtime_per_100_missed as u128
                            / 100) as u64
                    }
                };
                total_penalty = total_penalty.saturating_add(downtime_penalty);
            }

            // 3. Process non-downtime offenses normally (each counts)
            for evidence in evidence_list {
                let stake_penalty = match evidence.offense {
                    SlashingOffense::DoubleBlock { .. } => {
                        (original_stake as u128 * params.slashing_percentage_double_sign as u128
                            / 100) as u64
                    }
                    SlashingOffense::DoubleVote { .. } => {
                        (original_stake as u128 * params.slashing_percentage_double_vote as u128
                            / 100) as u64
                    }
                    SlashingOffense::Downtime { .. } => {
                        0 // Already handled above via worst-entry
                    }
                    SlashingOffense::InvalidStateTransition { .. } => {
                        (original_stake as u128 * params.slashing_percentage_invalid_state as u128
                            / 100) as u64
                    }
                    SlashingOffense::Censorship { .. } => {
                        (original_stake as u128 * params.slashing_percentage_censorship as u128
                            / 100) as u64
                    }
                    SlashingOffense::Collusion { .. } => original_stake,
                };

                total_penalty = total_penalty.saturating_add(stake_penalty);
            }
        }

        // GRANT-PROTECT: Cap penalty so stake never drops below MIN_VALIDATOR_STAKE.
        // Bootstrap-granted validators (100K MOLT) have a 25K buffer — that is the
        // maximum that can ever be slashed economically.  This prevents the chain
        // from stranding validators at 0 stake where the liveness fallback kicks in
        // and blocks are produced by validators with no skin-in-the-game.
        let slash_budget = original_stake.saturating_sub(MIN_VALIDATOR_STAKE);
        let capped_penalty = total_penalty.min(slash_budget);
        let total_slashed = if capped_penalty > 0 {
            stake_pool.slash_validator(validator, capped_penalty)
        } else {
            0
        };

        if total_slashed > 0 {
            self.slash(validator, current_slot);
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
    fn test_weighted_halts_when_no_min_stake() {
        let mut set = ValidatorSet::new();
        let pool = StakePool::new(); // empty pool — no validator meets MIN_VALIDATOR_STAKE
        let pk1 = Pubkey::new([1u8; 32]);

        set.add_validator(ValidatorInfo::new(pk1, 0));

        // MIN-STAKE GUARD: With no validators meeting MIN_VALIDATOR_STAKE,
        // leader selection returns None (chain halts) — never produces blocks
        // with 0-stake validators.
        let leader = set.select_leader_weighted(0, &pool);
        assert_eq!(
            leader, None,
            "Should return None when no validator meets MIN_VALIDATOR_STAKE"
        );
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

    /// A5-01: Verify that mixing a randomness seed (previous block hash)
    /// changes leader selection output and that the same seed is deterministic.
    #[test]
    fn test_weighted_leader_selection_with_seed() {
        let mut set = ValidatorSet::new();
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);
        let pk3 = Pubkey::new([3u8; 32]);

        set.add_validator(ValidatorInfo::new(pk1, 0));
        set.add_validator(ValidatorInfo::new(pk2, 0));
        set.add_validator(ValidatorInfo::new(pk3, 0));

        pool.stake(pk1, 100_000_000_000_000, 0).unwrap();
        pool.stake(pk2, 150_000_000_000_000, 0).unwrap();
        pool.stake(pk3, 120_000_000_000_000, 0).unwrap();

        let seed_a = [0xAA; 32];
        let seed_b = [0xBB; 32];

        // Same slot + same seed → deterministic
        let r1 = set.select_leader_weighted_with_seed(42, &pool, &seed_a);
        let r2 = set.select_leader_weighted_with_seed(42, &pool, &seed_a);
        assert_eq!(r1, r2, "Same slot and seed must produce same leader");

        // Different seeds may produce different leader distribution
        // Run over many slots — with different seeds, distributions should differ
        let mut results_a = std::collections::HashMap::new();
        let mut results_b = std::collections::HashMap::new();
        for slot in 0..200 {
            if let Some(pk) = set.select_leader_weighted_with_seed(slot, &pool, &seed_a) {
                *results_a.entry(pk).or_insert(0u32) += 1;
            }
            if let Some(pk) = set.select_leader_weighted_with_seed(slot, &pool, &seed_b) {
                *results_b.entry(pk).or_insert(0u32) += 1;
            }
        }
        // Seeds should produce different distributions (extremely unlikely to be identical)
        assert_ne!(
            results_a, results_b,
            "Different seeds must produce different leader distributions"
        );

        // Empty seed should match select_leader_weighted (backward compat)
        for slot in 0..50 {
            let no_seed = set.select_leader_weighted(slot, &pool);
            let empty_seed = set.select_leader_weighted_with_seed(slot, &pool, &[]);
            assert_eq!(
                no_seed, empty_seed,
                "Empty seed must match no-seed variant at slot {}",
                slot
            );
        }
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

    /// A5-02: Fork choice prefers block with higher cumulative stake weight
    /// even when competing blocks are at the same slot.
    #[test]
    fn test_fork_choice_cumulative_weight_resolution() {
        let mut fc = ForkChoice::new();
        let block_a = Hash::new([0xAA; 32]);
        let block_b = Hash::new([0xBB; 32]);

        // Block A gets 3 attestations (total weight 60)
        fc.add_head(100, block_a, 20);
        fc.add_head(100, block_a, 15);
        fc.add_head(100, block_a, 25);

        // Block B gets 2 attestations (total weight 80)
        fc.add_head(100, block_b, 50);
        fc.add_head(100, block_b, 30);

        // Block B should win despite fewer attestations (80 > 60)
        let (_, selected) = fc.select_head().unwrap();
        assert_eq!(
            selected, block_b,
            "Fork choice must prefer higher cumulative weight"
        );
        assert_eq!(fc.get_weight(&block_a), 60);
        assert_eq!(fc.get_weight(&block_b), 80);
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

    // ================================================================
    // Graduation System tests
    // ================================================================

    #[test]
    fn test_bootstrap_grant_first_200() {
        let mut pool = StakePool::new();

        // Issue 200 bootstrap grants — all should succeed
        for i in 0..200u64 {
            let pk = Pubkey::new({
                let mut b = [0u8; 32];
                b[..8].copy_from_slice(&i.to_le_bytes());
                b
            });
            let idx = pool.next_bootstrap_index().unwrap();
            assert_eq!(idx, i);
            pool.stake_with_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, idx)
                .unwrap();

            // Confirm bootstrap debt exists
            let stake = pool.get_stake(&pk).unwrap();
            assert_eq!(stake.bootstrap_debt, BOOTSTRAP_GRANT_AMOUNT);
            assert_eq!(stake.bootstrap_index, i);
            assert_eq!(stake.status, BootstrapStatus::Bootstrapping);
        }

        assert_eq!(pool.bootstrap_grants_issued(), 200);

        // 201st grant should return None
        assert!(pool.next_bootstrap_index().is_none());

        // Self-funded validator (#201) — no debt
        let pk_201 = Pubkey::new([0xFFu8; 32]);
        pool.stake_with_index(pk_201, MIN_VALIDATOR_STAKE, 0, u64::MAX)
            .unwrap();
        let stake_201 = pool.get_stake(&pk_201).unwrap();
        assert_eq!(stake_201.bootstrap_debt, 0);
        assert_eq!(stake_201.status, BootstrapStatus::FullyVested);
        assert_eq!(stake_201.bootstrap_index, u64::MAX);
    }

    #[test]
    fn test_standard_graduation_50_50_split() {
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        assert_eq!(stake.bootstrap_debt, BOOTSTRAP_GRANT_AMOUNT);
        assert_eq!(stake.status, BootstrapStatus::Bootstrapping);

        // Simulate low uptime: 0 blocks produced on a 1-validator network.
        // With the plan formula: expected_blocks = slots_active / num_validators.
        // At slot SLOTS_PER_EPOCH * 10, 1 validator: expected = SLOTS_PER_EPOCH * 10.
        // 0 blocks → uptime = 0 → standard 50/50 split.
        let claim_slot = SLOTS_PER_EPOCH * 10;
        stake.add_reward(1_000_000_000, claim_slot); // 1 MOLT
        stake.blocks_produced = 0; // Zero uptime — definitely below 95%
        let (liquid, debt) = stake.claim_rewards(claim_slot, 1);

        // Standard 50/50 split
        assert_eq!(debt, 500_000_000); // 0.5 MOLT to debt
        assert_eq!(liquid, 500_000_000); // 0.5 MOLT liquid
        assert_eq!(stake.bootstrap_debt, BOOTSTRAP_GRANT_AMOUNT - 500_000_000);
        assert_eq!(stake.earned_amount, 500_000_000);
        assert_eq!(stake.total_claimed, 1_000_000_000);
        assert_eq!(stake.total_debt_repaid, 500_000_000);
    }

    #[test]
    fn test_performance_bonus_75_25_split() {
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        // Simulate high uptime on a 1-validator network.
        // Plan formula: expected_blocks = (current_slot - start_slot) / num_validators
        //   = SLOTS_PER_EPOCH / 1 = 432,000
        // Need uptime_bps >= 9500 → blocks_produced * 10000 / 432,000 >= 9500
        //   → blocks_produced >= 432,000 * 9500 / 10000 = 410,400
        let num_validators: u64 = 1;
        let test_slot = SLOTS_PER_EPOCH;
        let expected_blocks = test_slot / num_validators;
        // Produce 95% of expected blocks
        stake.blocks_produced = expected_blocks * 95 / 100;
        stake.add_reward(1_000_000_000, test_slot); // 1 MOLT

        let uptime = stake.uptime_bps(test_slot, num_validators);
        assert!(
            uptime >= UPTIME_BONUS_THRESHOLD_BPS,
            "uptime {} should be >= {} (blocks={}, expected={})",
            uptime,
            UPTIME_BONUS_THRESHOLD_BPS,
            stake.blocks_produced,
            expected_blocks,
        );

        let (liquid, debt) = stake.claim_rewards(test_slot, num_validators);

        // Performance bonus: 75/25 split
        assert_eq!(debt, 750_000_000); // 0.75 MOLT to debt
        assert_eq!(liquid, 250_000_000); // 0.25 MOLT liquid (total - paid)
        assert_eq!(stake.total_debt_repaid, 750_000_000);
    }

    #[test]
    fn test_time_cap_graduation() {
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        assert_eq!(stake.status, BootstrapStatus::Bootstrapping);
        assert_eq!(stake.bootstrap_debt, BOOTSTRAP_GRANT_AMOUNT);

        // Add rewards but don't claim enough to fully repay
        let reward = 10_000_000_000_000u64; // 10,000 MOLT
        stake.add_reward(reward, MAX_BOOTSTRAP_SLOTS);

        // Claim at exactly the time cap
        let (liquid, debt) = stake.claim_rewards(MAX_BOOTSTRAP_SLOTS, 1);

        // Time cap reached: entire reward is liquid, debt is forgiven
        assert_eq!(liquid, reward);
        assert_eq!(debt, 0);
        assert_eq!(stake.bootstrap_debt, 0);
        assert_eq!(stake.status, BootstrapStatus::FullyVested);
        assert!(stake.graduation_slot.is_some());
        assert_eq!(stake.graduation_slot.unwrap(), MAX_BOOTSTRAP_SLOTS);
    }

    #[test]
    fn test_time_cap_before_debt_repayment() {
        let pk = Pubkey::new([1u8; 32]);
        let start_slot = 1000;
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, start_slot, 0);

        // Add a small reward — nowhere near enough to repay debt
        stake.add_reward(1_000_000_000, start_slot + 100); // 1 MOLT

        // Claim before time cap — normal 50/50 split (0 blocks → 0 uptime)
        let (liquid1, debt1) = stake.claim_rewards(start_slot + 100, 1);
        assert_eq!(liquid1, 500_000_000);
        assert_eq!(debt1, 500_000_000);
        assert_eq!(stake.status, BootstrapStatus::Bootstrapping);

        // Now add another small reward and claim PAST the time cap
        stake.add_reward(2_000_000_000, start_slot + MAX_BOOTSTRAP_SLOTS + 1);
        let (liquid2, debt2) = stake.claim_rewards(start_slot + MAX_BOOTSTRAP_SLOTS + 1, 1);

        // Time cap reached: debt forgiven, full reward is liquid
        assert_eq!(liquid2, 2_000_000_000);
        assert_eq!(debt2, 0);
        assert_eq!(stake.bootstrap_debt, 0);
        assert_eq!(stake.status, BootstrapStatus::FullyVested);
    }

    #[test]
    fn test_fingerprint_blocks_duplicate() {
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);

        pool.stake_with_index(pk1, BOOTSTRAP_GRANT_AMOUNT, 0, 0)
            .unwrap();
        pool.stake_with_index(pk2, BOOTSTRAP_GRANT_AMOUNT, 0, 1)
            .unwrap();

        let fingerprint = [0xABu8; 32];

        // First registration should succeed
        assert!(pool.register_fingerprint(&pk1, fingerprint).is_ok());

        // Same fingerprint for different validator should fail
        let err = pool.register_fingerprint(&pk2, fingerprint).unwrap_err();
        assert!(err.contains("already registered"));

        // Same validator re-registering same fingerprint should be idempotent
        assert!(pool.register_fingerprint(&pk1, fingerprint).is_ok());
    }

    #[test]
    fn test_fingerprint_zero_always_accepted() {
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);

        pool.stake_with_index(pk1, BOOTSTRAP_GRANT_AMOUNT, 0, 0)
            .unwrap();
        pool.stake_with_index(pk2, BOOTSTRAP_GRANT_AMOUNT, 0, 1)
            .unwrap();

        let zero_fp = [0u8; 32];

        // Zero fingerprints should always be accepted (dev mode / legacy)
        assert!(pool.register_fingerprint(&pk1, zero_fp).is_ok());
        assert!(pool.register_fingerprint(&pk2, zero_fp).is_ok());
    }

    #[test]
    fn test_machine_migration() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake_with_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0)
            .unwrap();

        let old_fp = [0xAAu8; 32];
        let new_fp = [0xBBu8; 32];

        pool.register_fingerprint(&pk, old_fp).unwrap();

        // Migrate to new machine
        let migrate_slot = MIGRATION_COOLDOWN_SLOTS + 100;
        pool.migrate_fingerprint(&pk, new_fp, migrate_slot).unwrap();

        // Old fingerprint should be released
        assert!(pool.fingerprint_owner(&old_fp).is_none());
        // New fingerprint should be registered
        assert_eq!(pool.fingerprint_owner(&new_fp), Some(&pk));

        // Immediately migrating again should fail (cooldown)
        let newer_fp = [0xCCu8; 32];
        let err = pool
            .migrate_fingerprint(&pk, newer_fp, migrate_slot + 100)
            .unwrap_err();
        assert!(err.contains("cooldown"));

        // After cooldown, migration should succeed
        let after_cooldown = migrate_slot + MIGRATION_COOLDOWN_SLOTS + 1;
        pool.migrate_fingerprint(&pk, newer_fp, after_cooldown)
            .unwrap();
        assert_eq!(pool.fingerprint_owner(&newer_fp), Some(&pk));
        assert!(pool.fingerprint_owner(&new_fp).is_none());
    }

    #[test]
    fn test_graduation_backward_compat() {
        // Old StakeInfo::new() still works — creates with u64::MAX index
        let pk = Pubkey::new([1u8; 32]);
        let stake = StakeInfo::new(pk, MIN_VALIDATOR_STAKE, 0);

        // u64::MAX index → no bootstrap debt (self-funded / legacy behavior)
        assert_eq!(stake.bootstrap_index, u64::MAX);
        assert_eq!(stake.bootstrap_debt, 0);
        assert_eq!(stake.status, BootstrapStatus::FullyVested);
        assert_eq!(stake.machine_fingerprint, [0u8; 32]);
        assert_eq!(stake.start_slot, 0);
        assert_eq!(stake.last_migration_slot, 0);
    }

    #[test]
    fn test_uptime_bps_calculation() {
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        // 0 blocks produced, 1 validator → 0 uptime
        assert_eq!(stake.uptime_bps(SLOTS_PER_EPOCH, 1), 0);

        // Plan formula: expected_blocks = slots_active / num_validators
        // With 1 validator over 10 epochs: expected = SLOTS_PER_EPOCH * 10 / 1 = 4,320,000
        // 10 blocks / 4,320,000 expected = 0 bps (truncated)
        stake.blocks_produced = 10;
        let uptime_10e = stake.uptime_bps(SLOTS_PER_EPOCH * 10, 1);
        assert_eq!(uptime_10e, 0); // 10 blocks out of 4.32M expected is negligible

        // With 200 validators over 10 epochs: expected = SLOTS_PER_EPOCH * 10 / 200 = 21,600
        // 10 blocks / 21,600 = 4 bps (still very low)
        let uptime_200v = stake.uptime_bps(SLOTS_PER_EPOCH * 10, 200);
        assert_eq!(uptime_200v, 4);

        // Realistic: 200 validators, 10 epochs. Each expected ≈ 21,600 blocks.
        // A validator producing 20,520 blocks (95%) should get 9500 bps.
        stake.blocks_produced = 20_520;
        let uptime_95 = stake.uptime_bps(SLOTS_PER_EPOCH * 10, 200);
        assert!(
            uptime_95 >= 9500,
            "uptime {} should be >= 9500 at 95%",
            uptime_95
        );

        // Saturates at 10000
        stake.blocks_produced = 100_000;
        assert_eq!(stake.uptime_bps(SLOTS_PER_EPOCH * 10, 200), 10000);

        // Edge: 0 slots active → 0 uptime
        assert_eq!(stake.uptime_bps(0, 1), 0);
    }

    #[test]
    fn test_self_funded_validator_no_debt() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);

        // Exhaust bootstrap grants
        for i in 0..200 {
            let dummy = Pubkey::new({
                let mut b = [0u8; 32];
                b[..8].copy_from_slice(&(i as u64).to_le_bytes());
                b[31] = 0xFE; // Different from pk
                b
            });
            let idx = pool.next_bootstrap_index().unwrap();
            pool.stake_with_index(dummy, BOOTSTRAP_GRANT_AMOUNT, 0, idx)
                .unwrap();
        }

        // Now #201 — must self-fund
        assert!(pool.next_bootstrap_index().is_none());
        pool.stake_with_index(pk, MIN_VALIDATOR_STAKE, 0, u64::MAX)
            .unwrap();

        let stake = pool.get_stake(&pk).unwrap();
        assert_eq!(stake.bootstrap_debt, 0);
        assert_eq!(stake.status, BootstrapStatus::FullyVested);

        // Can immediately unstake (no vesting requirement)
        let result = pool.request_unstake(&pk, MIN_VALIDATOR_STAKE / 2, 1000, pk);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bootstrap_counter_persists() {
        // Verify bootstrap_grants_issued survives serialization roundtrip
        // Production uses bincode (binary) — test both bincode and JSON
        let mut pool = StakePool::new();
        for i in 0..5u64 {
            let pk = Pubkey::new({
                let mut b = [0u8; 32];
                b[..8].copy_from_slice(&i.to_le_bytes());
                b
            });
            let idx = pool.next_bootstrap_index().unwrap();
            pool.stake_with_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, idx)
                .unwrap();
        }
        assert_eq!(pool.bootstrap_grants_issued(), 5);

        // Bincode roundtrip (production format — used by RocksDB persistence)
        let bytes = bincode::serialize(&pool).unwrap();
        let pool2: StakePool = bincode::deserialize(&bytes).unwrap();
        assert_eq!(pool2.bootstrap_grants_issued(), 5);

        // The counter continues from 5, not 0
        let mut pool2 = pool2;
        let idx = pool2.next_bootstrap_index().unwrap();
        assert_eq!(idx, 5);
        assert_eq!(pool2.bootstrap_grants_issued(), 6);

        // Bincode roundtrip WITH fingerprint (tests fingerprint_registry serialization)
        let pk_with_fp = Pubkey::new([0xAA; 32]);
        let next_idx = pool.next_bootstrap_index().unwrap();
        pool.stake_with_index(pk_with_fp, BOOTSTRAP_GRANT_AMOUNT, 0, next_idx)
            .unwrap();
        pool.register_fingerprint(&pk_with_fp, [0xFF; 32]).unwrap();
        let bytes2 = bincode::serialize(&pool).unwrap();
        let pool3: StakePool = bincode::deserialize(&bytes2).unwrap();
        assert_eq!(pool3.bootstrap_grants_issued(), 6); // 5 original + 1 new
        assert_eq!(pool3.fingerprint_owner(&[0xFF; 32]), Some(&pk_with_fp));
    }

    #[test]
    fn test_fingerprint_allows_self_funded() {
        // Duplicate fingerprint is OK for self-funded validators (201+)
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);
        let shared_fp = [0xAA; 32];

        // pk1 is bootstrap (index 0) with fingerprint
        pool.stake_with_index(pk1, BOOTSTRAP_GRANT_AMOUNT, 0, 0)
            .unwrap();
        pool.register_fingerprint(&pk1, shared_fp).unwrap();

        // pk2 is self-funded (index u64::MAX) — same fingerprint
        pool.stake_with_index(pk2, MIN_VALIDATOR_STAKE, 0, u64::MAX)
            .unwrap();
        // register_fingerprint should fail (fingerprint taken by pk1)
        let err = pool.register_fingerprint(&pk2, shared_fp).unwrap_err();
        assert!(err.contains("already registered"));

        // But pk2 is still staked and operational (self-funded, no fingerprint)
        let stake2 = pool.get_stake(&pk2).unwrap();
        assert_eq!(stake2.bootstrap_debt, 0); // self-funded, no debt
        assert_eq!(stake2.status, BootstrapStatus::FullyVested);
    }

    #[test]
    fn test_try_bootstrap_with_fingerprint_atomic() {
        // Verifies the atomic method: fingerprint fails → index NOT consumed
        let mut pool = StakePool::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);
        let fingerprint = [0xBB; 32];

        // pk1 bootstraps successfully with fingerprint
        let (idx1, is_new1) = pool
            .try_bootstrap_with_fingerprint(pk1, BOOTSTRAP_GRANT_AMOUNT, 0, fingerprint)
            .unwrap();
        assert_eq!(idx1, 0);
        assert!(is_new1);
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // pk2 tries same fingerprint → should fail
        let err = pool
            .try_bootstrap_with_fingerprint(pk2, BOOTSTRAP_GRANT_AMOUNT, 0, fingerprint)
            .unwrap_err();
        assert!(err.contains("already registered"));

        // Counter should still be 1 (NOT 2) — index was NOT wasted
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // pk2 with a different fingerprint succeeds
        let fp2 = [0xCC; 32];
        let (idx2, is_new2) = pool
            .try_bootstrap_with_fingerprint(pk2, BOOTSTRAP_GRANT_AMOUNT, 0, fp2)
            .unwrap();
        assert_eq!(idx2, 1);
        assert!(is_new2);
        assert_eq!(pool.bootstrap_grants_issued(), 2);
    }

    #[test]
    fn test_try_bootstrap_existing_validator_idempotent() {
        // Duplicate bootstrap for same validator is idempotent (no accumulation)
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        let fp = [0xDD; 32];

        // First bootstrap
        let (idx, is_new) = pool
            .try_bootstrap_with_fingerprint(pk, BOOTSTRAP_GRANT_AMOUNT, 0, fp)
            .unwrap();
        assert_eq!(idx, 0);
        assert!(is_new);

        // Second bootstrap attempt — idempotent, no accumulation
        let (idx2, is_new2) = pool
            .try_bootstrap_with_fingerprint(pk, BOOTSTRAP_GRANT_AMOUNT, 100, fp)
            .unwrap();
        assert_eq!(idx2, 0); // Same index
        assert!(!is_new2); // Not new

        // Counter should be 1 (only one grant)
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // Stake stays at BOOTSTRAP_GRANT_AMOUNT (not doubled)
        let stake = pool.get_stake(&pk).unwrap();
        assert_eq!(stake.amount, BOOTSTRAP_GRANT_AMOUNT);
    }

    #[test]
    fn test_performance_bonus_uses_constant() {
        // Verify PERFORMANCE_BONUS_BPS = 15000 produces 75% debt fraction
        // base_half = reward / 2, debt = base_half * 15000 / 10000 = base_half * 1.5
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);
        let reward: u64 = 1_000_000_000; // 1 MOLT
        stake.rewards_earned = reward;

        // Give enough blocks for 100% uptime with 1 validator over 10 epochs.
        // expected_blocks = SLOTS_PER_EPOCH * 10 / 1 = 4,320,000
        // 1M blocks / 4.32M → only ~2314 bps, NOT enough for bonus.
        // Need >= 4,104,000 blocks (95%) for a 1-validator network.
        let num_validators: u64 = 1;
        let test_slot = SLOTS_PER_EPOCH * 10;
        let expected_blocks = test_slot / num_validators;
        stake.blocks_produced = expected_blocks; // Exactly 100% uptime

        let (liquid, debt) = stake.claim_rewards(test_slot, num_validators);
        // debt = (reward / 2) * 15000 / 10000 = 500M * 1.5 = 750M
        assert_eq!(debt, 750_000_000);
        assert_eq!(liquid, 250_000_000);
        assert_eq!(liquid + debt, reward);
    }

    // ================================================================
    // K1-02: Fork handling with real Block objects
    // ================================================================

    #[test]
    fn test_fork_choice_with_real_blocks_same_slot() {
        // Two competing blocks at the same slot — heavier stake wins
        let mut fc = ForkChoice::new();
        let validator_a = [0xAAu8; 32];
        let validator_b = [0xBBu8; 32];

        let block_a = crate::Block::new_with_timestamp(
            10,
            Hash::default(),
            Hash::default(),
            validator_a,
            Vec::new(),
            1000,
        );
        let block_b = crate::Block::new_with_timestamp(
            10,
            Hash::default(),
            Hash::default(),
            validator_b,
            Vec::new(),
            1001,
        );
        let hash_a = block_a.hash();
        let hash_b = block_b.hash();

        // Block A has less stake weight
        fc.add_head(block_a.header.slot, hash_a, 100);
        // Block B has more stake weight → wins
        fc.add_head(block_b.header.slot, hash_b, 200);

        let (slot, selected) = fc.select_head().unwrap();
        assert_eq!(slot, 10);
        assert_eq!(selected, hash_b, "Block B should win with higher stake");
    }

    #[test]
    fn test_fork_choice_higher_slot_wins_over_heavier() {
        // A fork at slot 11 should win over a heavier fork at slot 10
        let mut fc = ForkChoice::new();

        let block_low = crate::Block::new_with_timestamp(
            10,
            Hash::default(),
            Hash::default(),
            [1u8; 32],
            Vec::new(),
            1000,
        );
        let block_high = crate::Block::new_with_timestamp(
            11,
            Hash::default(),
            Hash::default(),
            [2u8; 32],
            Vec::new(),
            1001,
        );

        // Low slot has much more weight
        fc.add_head(block_low.header.slot, block_low.hash(), 1000);
        // High slot has less weight but higher slot → wins
        fc.add_head(block_high.header.slot, block_high.hash(), 50);

        let (slot, _) = fc.select_head().unwrap();
        assert_eq!(slot, 11, "Higher slot should win regardless of weight");
    }

    #[test]
    fn test_fork_choice_deterministic_tiebreak() {
        // Same slot, same weight → deterministic hash comparison decides
        let mut fc = ForkChoice::new();

        let block_a = crate::Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::new([1u8; 32]),
            [0xAA; 32],
            Vec::new(),
            100,
        );
        let block_b = crate::Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::new([2u8; 32]),
            [0xBB; 32],
            Vec::new(),
            101,
        );

        let hash_a = block_a.hash();
        let hash_b = block_b.hash();

        // Same slot, same weight
        fc.add_head(5, hash_a, 100);
        fc.add_head(5, hash_b, 100);

        let (_, selected) = fc.select_head().unwrap();
        // Tiebreak: deterministic — same every time
        let expected = if hash_a.0 > hash_b.0 { hash_a } else { hash_b };
        assert_eq!(
            selected, expected,
            "Deterministic tiebreak should pick consistently"
        );

        // Run again — same result
        let (_, selected2) = fc.select_head().unwrap();
        assert_eq!(selected, selected2, "Tiebreak must be deterministic");
    }

    #[test]
    fn test_fork_choice_late_attestations_flip_preference() {
        // Block A initially leads, but late attestations for B flip the decision
        let mut fc = ForkChoice::new();
        let hash_a = Hash::new([0xAA; 32]);
        let hash_b = Hash::new([0xBB; 32]);

        // Initial: A leads
        fc.add_head(20, hash_a, 100);
        fc.add_head(20, hash_b, 50);

        let (_, selected) = fc.select_head().unwrap();
        assert_eq!(selected, hash_a, "A should lead initially");

        // Late attestations arrive for B
        fc.add_head(20, hash_b, 80);

        // Now B leads (50 + 80 = 130 > 100)
        let (_, selected) = fc.select_head().unwrap();
        assert_eq!(selected, hash_b, "Late attestations should flip to B");
    }

    #[test]
    fn test_fork_choice_multi_fork_three_candidates() {
        // Three-way fork at same slot — heaviest wins
        let mut fc = ForkChoice::new();
        let hash_a = Hash::new([0xAA; 32]);
        let hash_b = Hash::new([0xBB; 32]);
        let hash_c = Hash::new([0xCC; 32]);

        fc.add_head(50, hash_a, 100);
        fc.add_head(50, hash_b, 200);
        fc.add_head(50, hash_c, 150);

        let (_, selected) = fc.select_head().unwrap();
        assert_eq!(selected, hash_b, "Heaviest block should win in 3-way fork");
    }

    #[test]
    fn test_fork_choice_finality_prevents_reorg() {
        // After finality, adding a heavier fork at a finalized slot should
        // not win if the non-finalized fork has a later slot.
        // This tests that slot priority (representing chain length) matters
        // more than weight at finalized depths.
        let mut fc = ForkChoice::new();
        let finalized_hash = Hash::new([0x11; 32]);
        let extension_hash = Hash::new([0x22; 32]);
        let attacker_hash = Hash::new([0xFF; 32]);

        // Chain: finalized at slot 100, extended to slot 110
        fc.add_head(100, finalized_hash, 1000);
        fc.add_head(110, extension_hash, 50);

        // Attacker tries to reorg at slot 100 with massive weight
        fc.add_head(100, attacker_hash, 5000);

        let (slot, selected) = fc.select_head().unwrap();
        // The extension at slot 110 should still be canonical (higher slot wins)
        assert_eq!(slot, 110, "Extension should win — slot priority");
        assert_eq!(selected, extension_hash);
    }

    /// P9-CORE-02: SlashingEvidence uses deterministic block_timestamp, not SystemTime::now()
    #[test]
    fn test_slashing_evidence_deterministic_timestamp() {
        let v = Pubkey::new([1u8; 32]);
        let r = Pubkey::new([2u8; 32]);
        let ts = 1_700_000_040u64;
        let e1 = SlashingEvidence::new(
            SlashingOffense::DoubleBlock {
                slot: 100,
                block_hash_1: Hash::new([0xAA; 32]),
                block_hash_2: Hash::new([0xBB; 32]),
            },
            v,
            100,
            r,
            ts,
        );
        let e2 = SlashingEvidence::new(
            SlashingOffense::DoubleBlock {
                slot: 100,
                block_hash_1: Hash::new([0xAA; 32]),
                block_hash_2: Hash::new([0xBB; 32]),
            },
            v,
            100,
            r,
            ts,
        );
        assert_eq!(
            e1.timestamp, ts,
            "timestamp must equal caller-supplied value"
        );
        assert_eq!(
            e1.timestamp, e2.timestamp,
            "two evidence structs with same input must be identical"
        );
    }

    // ================================================================
    // GRANT-PROTECT: Slashing cannot take stake below MIN_VALIDATOR_STAKE
    // ================================================================

    #[test]
    fn test_grant_protection_double_vote_cannot_slash_below_min_stake() {
        // Simulate the exact scenario that was killing validators:
        // A bootstrap-granted validator (100K MOLT) receives multiple
        // DoubleVote evidence (30% each). Without protection, 4 events
        // would slash to 0. With protection, stake stays at MIN_VALIDATOR_STAKE.
        let mut tracker = SlashingTracker::new();
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        let reporter = Pubkey::new([2u8; 32]);
        let params = ConsensusParams::default();

        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap(); // 100K MOLT
        let initial = pool.get_stake(&pk).unwrap().total_stake();
        assert_eq!(initial, BOOTSTRAP_GRANT_AMOUNT);

        // Add 4 DoubleVote evidence events (each 30% = 120% total without cap)
        // Push directly to bypass signature verification (test-only)
        let evidence_list = tracker.evidence.entry(pk).or_default();
        for slot in 0..4u64 {
            let vote_1 = Vote::new(slot, Hash::new([0xAA; 32]), pk, [0u8; 64]);
            let vote_2 = Vote::new(slot, Hash::new([0xBB; 32]), pk, [0u8; 64]);
            evidence_list.push(SlashingEvidence::new(
                SlashingOffense::DoubleVote {
                    slot,
                    vote_1,
                    vote_2,
                },
                pk,
                slot,
                reporter,
                1_700_000_000 + slot,
            ));
        }

        let slashed = tracker.apply_economic_slashing_with_params(&pk, &mut pool, &params, 100);

        let remaining = pool.get_stake(&pk).unwrap().total_stake();
        assert!(
            remaining >= MIN_VALIDATOR_STAKE,
            "Stake ({}) must never drop below MIN_VALIDATOR_STAKE ({})",
            remaining,
            MIN_VALIDATOR_STAKE
        );

        // The max slashable is the buffer: 100K - 75K = 25K MOLT
        let max_slashable = BOOTSTRAP_GRANT_AMOUNT - MIN_VALIDATOR_STAKE;
        assert_eq!(
            slashed, max_slashable,
            "Should slash exactly the 25K buffer, not more"
        );
    }

    // ================================================================
    // Tiered Downtime Slashing System tests
    // ================================================================

    #[test]
    fn test_tiered_downtime_tier1_reputation_only() {
        // Tier 1 (1st offense): No economic slash, reputation penalty only
        let mut tracker = SlashingTracker::new();
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Record first downtime offense → tier 1
        let tier = tracker.record_downtime_offense(&pk, 100);
        assert_eq!(tier, 1, "First offense should be tier 1");

        // Add downtime evidence
        let evidence = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 0,
                current_slot: 500,
                missed_slots: 500,
            },
            pk,
            500,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        tracker.add_evidence(evidence);

        // Apply slashing — should be 0 (tier 1 = no economic penalty)
        let params = crate::genesis::ConsensusParams::default();
        let slashed = tracker.apply_economic_slashing_with_params(&pk, &mut pool, &params, 500);
        assert_eq!(slashed, 0, "Tier 1 downtime should not slash any stake");

        // Stake should be unchanged
        let stake = pool.get_stake(&pk).unwrap();
        assert_eq!(stake.total_stake(), BOOTSTRAP_GRANT_AMOUNT);
    }

    #[test]
    fn test_tiered_downtime_tier2_small_slash() {
        // Tier 2 (2nd offense): 0.5% slash
        let mut tracker = SlashingTracker::new();
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Record two offenses to reach tier 2
        tracker.record_downtime_offense(&pk, 100);
        let tier = tracker.record_downtime_offense(&pk, 200);
        assert_eq!(tier, 2, "Second offense should be tier 2");

        // Add downtime evidence
        let evidence = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 0,
                current_slot: 500,
                missed_slots: 500,
            },
            pk,
            500,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        tracker.add_evidence(evidence);

        // Apply slashing — should be 0.5% of stake
        let params = crate::genesis::ConsensusParams::default();
        let slashed = tracker.apply_economic_slashing_with_params(&pk, &mut pool, &params, 500);
        let expected =
            (BOOTSTRAP_GRANT_AMOUNT as u128 * DOWNTIME_TIER2_SLASH_BPS as u128 / 10_000) as u64;
        assert_eq!(
            slashed, expected,
            "Tier 2 should slash 0.5% ({} shells), got {}",
            expected, slashed
        );
    }

    #[test]
    fn test_tiered_downtime_tier3_graduated() {
        // Tier 3+ (3rd offense): Full graduated slashing
        let mut tracker = SlashingTracker::new();
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Record three offenses to reach tier 3
        tracker.record_downtime_offense(&pk, 100);
        tracker.record_downtime_offense(&pk, 200);
        let tier = tracker.record_downtime_offense(&pk, 300);
        assert_eq!(tier, 3, "Third offense should be tier 3");

        // Add downtime evidence: 500 missed slots → 5 × 1% = 5%
        let evidence = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 0,
                current_slot: 500,
                missed_slots: 500,
            },
            pk,
            500,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        tracker.add_evidence(evidence);

        let params = crate::genesis::ConsensusParams::default();
        let slashed = tracker.apply_economic_slashing_with_params(&pk, &mut pool, &params, 500);
        // 500/100 = 5 periods × 1% per period = 5%
        let expected = (BOOTSTRAP_GRANT_AMOUNT as u128 * 5 / 100) as u64;
        assert_eq!(
            slashed, expected,
            "Tier 3 should apply graduated slashing (5% for 500 missed), got {}",
            slashed
        );
    }

    #[test]
    fn test_downtime_forgiveness_decay() {
        // After DOWNTIME_FORGIVENESS_SLOTS of no offenses, tier resets
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // Record 2 offenses
        tracker.record_downtime_offense(&pk, 100);
        tracker.record_downtime_offense(&pk, 200);
        assert_eq!(tracker.get_downtime_offense_tier(&pk, 200), 2);

        // Record a 3rd offense AFTER the forgiveness window → should reset to 1
        let forgiven_slot = 200 + DOWNTIME_FORGIVENESS_SLOTS + 1;
        let tier = tracker.record_downtime_offense(&pk, forgiven_slot);
        assert_eq!(
            tier, 1,
            "After forgiveness window, offense count should reset; tier should be 1"
        );
    }

    #[test]
    fn test_slash_does_not_reduce_bootstrap_debt() {
        // Slashing burns stake but must NOT reduce bootstrap debt
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);
        let original_debt = stake.bootstrap_debt;
        assert_eq!(original_debt, BOOTSTRAP_GRANT_AMOUNT);

        // Slash 10% of stake
        let slash_amount = BOOTSTRAP_GRANT_AMOUNT / 10;
        stake.slash(slash_amount);

        // Debt should be unchanged
        assert_eq!(
            stake.bootstrap_debt, original_debt,
            "Slashing must NOT reduce bootstrap debt (perverse incentive fix)"
        );
        // Stake should be reduced
        assert_eq!(
            stake.amount,
            BOOTSTRAP_GRANT_AMOUNT - slash_amount,
            "Stake should be reduced by slash amount"
        );
    }

    #[test]
    fn test_penalty_repayment_boost_90_10_split() {
        // Penalty boost: 90% to debt, 10% liquid
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        // Activate penalty boost (lasts until slot 100_000)
        stake.penalty_boost_until = 100_000;

        // Add 1 MOLT reward
        stake.add_reward(1_000_000_000, 500);
        let (liquid, debt) = stake.claim_rewards(500, 1);

        // 90% to debt, 10% liquid
        assert_eq!(debt, 900_000_000, "90% should go to debt repayment");
        assert_eq!(liquid, 100_000_000, "10% should be liquid");
    }

    #[test]
    fn test_penalty_repayment_boost_expires() {
        // Penalty boost expires after penalty_boost_until slot
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, BOOTSTRAP_GRANT_AMOUNT, 0, 0);

        // Set boost to expire at slot 1000
        stake.penalty_boost_until = 1000;

        // Claim BEFORE expiry → 90/10 split
        stake.add_reward(1_000_000_000, 500);
        let (liquid1, debt1) = stake.claim_rewards(500, 1);
        assert_eq!(debt1, 900_000_000, "Before expiry: 90% to debt");
        assert_eq!(liquid1, 100_000_000, "Before expiry: 10% liquid");

        // Claim AFTER expiry → normal 50/50 split (0 blocks = 0 uptime)
        stake.add_reward(1_000_000_000, 1500);
        let (liquid2, debt2) = stake.claim_rewards(1500, 1);
        assert_eq!(debt2, 500_000_000, "After expiry: 50% to debt (standard)");
        assert_eq!(liquid2, 500_000_000, "After expiry: 50% liquid (standard)");

        // penalty_boost_until should be cleared
        assert_eq!(
            stake.penalty_boost_until, 0,
            "Boost should be cleared after expiry"
        );
    }

    #[test]
    fn test_suspension_check() {
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // Not suspended initially
        assert!(
            !tracker.is_suspended(&pk, 0),
            "Should not be suspended initially"
        );

        // Suspend at slot 100
        tracker.suspend_validator(&pk, 100);

        // Suspended during window
        assert!(
            tracker.is_suspended(&pk, 100),
            "Should be suspended at suspension slot"
        );
        assert!(
            tracker.is_suspended(&pk, 100 + DOWNTIME_SUSPENSION_SLOTS - 1),
            "Should be suspended before window ends"
        );

        // Not suspended after window
        assert!(
            !tracker.is_suspended(&pk, 100 + DOWNTIME_SUSPENSION_SLOTS),
            "Should not be suspended after window expires"
        );
    }

    #[test]
    fn test_top_up_stake_recovery() {
        // A slashed validator can top up to recover above minimum
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Slash below MIN_VALIDATOR_STAKE (75K)
        let slash_amount = BOOTSTRAP_GRANT_AMOUNT - MIN_VALIDATOR_STAKE + 1_000_000_000; // Slash to below 75K
        pool.slash_validator(&pk, slash_amount);

        let stake_before = pool.get_stake(&pk).unwrap();
        assert!(
            stake_before.total_stake() < MIN_VALIDATOR_STAKE,
            "Should be below minimum after slashing"
        );

        // Top up to recover
        let top_up = 10_000_000_000_000; // 10K MOLT
        let result = pool.top_up_stake(&pk, top_up);
        assert!(result.is_ok(), "Top-up should succeed");

        let stake_after = pool.get_stake(&pk).unwrap();
        assert!(
            stake_after.total_stake() >= MIN_VALIDATOR_STAKE,
            "Should be above minimum after top-up"
        );
    }

    #[test]
    fn test_ghost_validator_cleanup() {
        // Ghost validators (inactive for too long) get removed
        let mut pool = StakePool::new();

        // Create 3 validators
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);
        let pk3 = Pubkey::new([3u8; 32]);
        pool.stake(pk1, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();
        pool.stake(pk2, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();
        pool.stake(pk3, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Slash pk2 to 0 (ghost)
        pool.slash_validator(&pk2, BOOTSTRAP_GRANT_AMOUNT);

        let stake2 = pool.get_stake(&pk2).unwrap();
        assert_eq!(
            stake2.total_stake(),
            0,
            "Fully slashed validator has 0 stake"
        );

        // Remove ghosts with 10K slot grace
        let removed = pool.remove_ghost_validators(20_000, 10_000);
        assert_eq!(removed.len(), 1, "Should remove 1 ghost validator");
        assert_eq!(removed[0], pk2, "Should remove the fully-slashed validator");

        // pk2 should be gone
        assert!(
            pool.get_stake(&pk2).is_none(),
            "Ghost validator should be removed"
        );

        // pk1 and pk3 should remain
        assert!(
            pool.get_stake(&pk1).is_some(),
            "Active validator should remain"
        );
        assert!(
            pool.get_stake(&pk3).is_some(),
            "Active validator should remain"
        );
    }

    #[test]
    fn test_clear_slashed_for_recovery() {
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // Add evidence so slash() succeeds (needs severity >= 70)
        let evidence = SlashingEvidence::new(
            SlashingOffense::DoubleBlock {
                slot: 100,
                block_hash_1: Hash::new([0xAA; 32]),
                block_hash_2: Hash::new([0xBB; 32]),
            },
            pk,
            100,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        tracker.add_evidence(evidence);

        // Mark as slashed
        assert!(
            tracker.slash(&pk, 100),
            "Should be able to slash with evidence"
        );
        assert!(tracker.is_slashed(&pk), "Should be slashed");

        // Clear slashed status for recovery
        tracker.clear_slashed(&pk);
        assert!(
            !tracker.is_slashed(&pk),
            "Should not be slashed after clearing"
        );
    }

    #[test]
    fn test_cleanup_expired_removes_old_data() {
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // Record offense and suspension
        tracker.record_downtime_offense(&pk, 100);
        tracker.suspend_validator(&pk, 100);

        // Cleanup at a slot well past all expiry windows
        let far_future = 100 + DOWNTIME_FORGIVENESS_SLOTS + DOWNTIME_SUSPENSION_SLOTS + 1;
        tracker.cleanup_expired(far_future);

        // Suspension should be removed (expired)
        assert!(
            !tracker.is_suspended(&pk, far_future),
            "Expired suspension should be cleaned up"
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_min_stake_vs_bootstrap_grant_separation() {
        // Verify MIN_VALIDATOR_STAKE < BOOTSTRAP_GRANT_AMOUNT (the 25% buffer)
        assert!(
            MIN_VALIDATOR_STAKE < BOOTSTRAP_GRANT_AMOUNT,
            "MIN_VALIDATOR_STAKE ({}) must be less than BOOTSTRAP_GRANT_AMOUNT ({})",
            MIN_VALIDATOR_STAKE,
            BOOTSTRAP_GRANT_AMOUNT
        );

        // Verify the exact values
        assert_eq!(MIN_VALIDATOR_STAKE, 75_000 * 1_000_000_000); // 75K MOLT
        assert_eq!(BOOTSTRAP_GRANT_AMOUNT, 100_000 * 1_000_000_000); // 100K MOLT

        // Verify a validator can be slashed by up to 25% and still be above minimum
        let max_survivable_slash = BOOTSTRAP_GRANT_AMOUNT - MIN_VALIDATOR_STAKE;
        assert_eq!(
            max_survivable_slash,
            25_000 * 1_000_000_000,
            "25K MOLT buffer between grant and minimum"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // AUDIT-FIX REGRESSION TESTS
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_audit_critical3_multi_evidence_no_inflation() {
        // CRITICAL #3: Multiple downtime evidence entries must NOT each add 0.5%.
        // With 3 downtime entries at Tier 2, penalty should be 0.5% total (not 1.5%).
        let mut tracker = SlashingTracker::new();
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Tier 2
        tracker.record_downtime_offense(&pk, 100);
        tracker.record_downtime_offense(&pk, 200);

        // Add 3 downtime evidence entries with DIFFERENT last_active_slots
        // (so they aren't deduped — simulates what could happen before LOW-12 fix)
        for i in 0..3 {
            let ev = SlashingEvidence::new(
                SlashingOffense::Downtime {
                    last_active_slot: i * 100,
                    current_slot: 500 + i,
                    missed_slots: 300 + i,
                },
                pk,
                500 + i,
                Pubkey::new([2u8; 32]),
                1700000000,
            );
            tracker.add_evidence(ev);
        }

        let params = crate::genesis::ConsensusParams::default();
        let slashed = tracker.apply_economic_slashing_with_params(&pk, &mut pool, &params, 500);
        let expected_single =
            (BOOTSTRAP_GRANT_AMOUNT as u128 * DOWNTIME_TIER2_SLASH_BPS as u128 / 10_000) as u64;

        assert_eq!(
            slashed, expected_single,
            "REGRESSION CRITICAL-3: Multiple downtime entries must slash exactly \
             0.5% total ({}), not per-entry. Got {}",
            expected_single, slashed
        );
    }

    #[test]
    fn test_audit_high4_no_escalation_without_new_evidence() {
        // HIGH #4: has_new_downtime_evidence must gate record_downtime_offense.
        // Calling has_new_downtime_evidence twice with no new evidence should
        // return false the second time.
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // Add one downtime evidence
        let ev = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 0,
                current_slot: 100,
                missed_slots: 100,
            },
            pk,
            100,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        tracker.add_evidence(ev);

        // First check: new evidence → true
        assert!(
            tracker.has_new_downtime_evidence(&pk),
            "First call should detect new downtime evidence"
        );

        // Second check: same evidence → false
        assert!(
            !tracker.has_new_downtime_evidence(&pk),
            "REGRESSION HIGH-4: Second call with no new evidence must return false"
        );

        // Add another evidence entry → should return true again
        let ev2 = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 200,
                current_slot: 300,
                missed_slots: 100,
            },
            pk,
            300,
            Pubkey::new([2u8; 32]),
            1700000001,
        );
        tracker.add_evidence(ev2);
        assert!(
            tracker.has_new_downtime_evidence(&pk),
            "After adding new evidence, should detect it"
        );
    }

    #[test]
    fn test_audit_high5_tier_with_forgiveness_decay() {
        // HIGH #5: get_downtime_offense_tier must apply forgiveness check.
        // After DOWNTIME_FORGIVENESS_SLOTS, effective tier should be 0.
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        tracker.record_downtime_offense(&pk, 100);
        tracker.record_downtime_offense(&pk, 200);

        // Before forgiveness: tier 2
        assert_eq!(
            tracker.get_downtime_offense_tier(&pk, 200),
            2,
            "Should be tier 2 before forgiveness"
        );

        // After forgiveness: tier 0
        let forgiven_slot = 200 + DOWNTIME_FORGIVENESS_SLOTS + 1;
        assert_eq!(
            tracker.get_downtime_offense_tier(&pk, forgiven_slot),
            0,
            "REGRESSION HIGH-5: After forgiveness window, effective tier must be 0"
        );
    }

    #[test]
    fn test_audit_critical2_slashed_validators_iter() {
        // CRITICAL #2: slashed_validators() must return all slashed pubkeys.
        let mut tracker = SlashingTracker::new();
        let pk1 = Pubkey::new([1u8; 32]);
        let pk2 = Pubkey::new([2u8; 32]);

        // Add evidence and slash two validators
        for pk in [pk1, pk2] {
            let ev = SlashingEvidence::new(
                SlashingOffense::DoubleBlock {
                    slot: 100,
                    block_hash_1: Hash::new([0xAA; 32]),
                    block_hash_2: Hash::new([0xBB; 32]),
                },
                pk,
                100,
                Pubkey::new([3u8; 32]),
                1700000000,
            );
            tracker.add_evidence(ev);
            tracker.slash(&pk, 100);
        }

        let slashed: Vec<_> = tracker.slashed_validators().collect();
        assert_eq!(slashed.len(), 2, "Should have 2 slashed validators");
        assert!(slashed.contains(&pk1));
        assert!(slashed.contains(&pk2));

        // Clear one
        tracker.clear_slashed(&pk1);
        let slashed: Vec<_> = tracker.slashed_validators().collect();
        assert_eq!(
            slashed.len(),
            1,
            "After clearing pk1, only pk2 should remain"
        );
        assert!(slashed.contains(&pk2));
    }

    #[test]
    fn test_audit_medium6_upsert_syncs_penalty_boost() {
        // MEDIUM #6: upsert_stake_full must sync penalty_boost_until.
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);

        // Local entry with no boost
        pool.stake(pk, BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();

        // Remote entry with penalty boost
        let mut remote = StakeInfo::new(pk, BOOTSTRAP_GRANT_AMOUNT, 0);
        remote.penalty_boost_until = 10000;

        pool.upsert_stake_full(remote);

        let local = pool.get_stake(&pk).unwrap();
        assert_eq!(
            local.penalty_boost_until, 10000,
            "REGRESSION MEDIUM-6: upsert_stake_full must sync penalty_boost_until"
        );
    }

    #[test]
    fn test_audit_medium9_params_double_vote_censorship() {
        // MEDIUM #9: DoubleVote and Censorship percentages must come from ConsensusParams.
        let params = crate::genesis::ConsensusParams::default();
        assert_eq!(
            params.slashing_percentage_double_vote, 30,
            "Default double vote slash should be 30%"
        );
        assert_eq!(
            params.slashing_percentage_censorship, 25,
            "Default censorship slash should be 25%"
        );
    }

    #[test]
    fn test_audit_low12_dedup_by_last_active_slot() {
        // LOW #12: Downtime evidence should dedup by last_active_slot, not missed_slots.
        let mut tracker = SlashingTracker::new();
        let pk = Pubkey::new([1u8; 32]);

        // First entry: downtime starting at last_active=50
        let ev1 = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 50,
                current_slot: 200,
                missed_slots: 150,
            },
            pk,
            200,
            Pubkey::new([2u8; 32]),
            1700000000,
        );
        assert!(tracker.add_evidence(ev1), "First entry should be accepted");

        // Second entry: same last_active_slot but different missed_slots → should be deduped
        let ev2 = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 50,
                current_slot: 300,
                missed_slots: 250,
            },
            pk,
            300,
            Pubkey::new([2u8; 32]),
            1700000001,
        );
        assert!(
            !tracker.add_evidence(ev2),
            "REGRESSION LOW-12: Same last_active_slot must be treated as duplicate"
        );

        // Different last_active_slot → separate downtime event → should be accepted
        let ev3 = SlashingEvidence::new(
            SlashingOffense::Downtime {
                last_active_slot: 500,
                current_slot: 600,
                missed_slots: 100,
            },
            pk,
            600,
            Pubkey::new([2u8; 32]),
            1700000002,
        );
        assert!(
            tracker.add_evidence(ev3),
            "Different last_active_slot should be accepted as new downtime event"
        );
    }

    #[test]
    fn test_audit_low13_penalty_boost_cleared_on_graduation() {
        // LOW #13: penalty_boost_until must be cleared on normal graduation (not just time-cap).
        let mut stake = StakeInfo::new(Pubkey::new([1u8; 32]), BOOTSTRAP_GRANT_AMOUNT, 0);
        stake.bootstrap_debt = 100; // Very small debt
        stake.rewards_earned = 1000; // More than enough to pay it off
        stake.penalty_boost_until = 999999; // Active boost

        let (liquid, paid) = stake.claim_rewards(100, 3);
        assert_eq!(paid, 100, "Should pay off all debt");
        assert_eq!(liquid, 900, "Rest should be liquid");
        assert_eq!(
            stake.penalty_boost_until, 0,
            "REGRESSION LOW-13: penalty_boost_until must be 0 after graduation"
        );
        assert_eq!(stake.bootstrap_debt, 0);
        assert!(matches!(stake.status, BootstrapStatus::FullyVested));
    }

    // ========================================================================
    // VoteAuthority Tests — verifies the Single Vote Gatekeeper pattern
    // ========================================================================

    #[test]
    fn test_vote_authority_first_vote_succeeds() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());
        let hash = Hash::new([42u8; 32]);

        let vote = va.try_vote(1, hash);
        assert!(vote.is_some(), "First vote for a slot must succeed");

        let v = vote.unwrap();
        assert_eq!(v.slot, 1);
        assert_eq!(v.block_hash, hash);
        assert_eq!(v.validator, kp.pubkey());
        assert!(v.verify(), "Vote signature must be valid");
    }

    #[test]
    fn test_vote_authority_same_hash_returns_none() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());
        let hash = Hash::new([42u8; 32]);

        let v1 = va.try_vote(1, hash);
        assert!(v1.is_some());

        // Same slot, same hash → benign P2P echo → returns None
        let v2 = va.try_vote(1, hash);
        assert!(
            v2.is_none(),
            "Second vote for same (slot, hash) must return None"
        );
    }

    #[test]
    fn test_vote_authority_different_hash_returns_none() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());
        let hash_a = Hash::new([42u8; 32]);
        let hash_b = Hash::new([99u8; 32]);

        let v1 = va.try_vote(1, hash_a);
        assert!(v1.is_some());

        // Same slot, DIFFERENT hash → DoubleVote attempt → REFUSED
        let v2 = va.try_vote(1, hash_b);
        assert!(v2.is_none(), "Equivocating vote must be REFUSED");
    }

    #[test]
    fn test_vote_authority_different_slots_succeed() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());

        let v1 = va.try_vote(1, Hash::new([1u8; 32]));
        let v2 = va.try_vote(2, Hash::new([2u8; 32]));
        let v3 = va.try_vote(3, Hash::new([3u8; 32]));

        assert!(v1.is_some());
        assert!(v2.is_some());
        assert!(v3.is_some());
        assert_eq!(va.voted_count(), 3);
    }

    #[test]
    fn test_vote_authority_has_voted() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());

        assert!(!va.has_voted(1));
        va.try_vote(1, Hash::new([1u8; 32]));
        assert!(va.has_voted(1));
        assert!(!va.has_voted(2));
    }

    #[test]
    fn test_vote_authority_prune() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());

        for slot in 1..=100 {
            va.try_vote(slot, Hash::new([slot as u8; 32]));
        }
        assert_eq!(va.voted_count(), 100);

        va.prune(51);
        assert_eq!(va.voted_count(), 50, "Prune should remove slots < 51");
        assert!(!va.has_voted(50));
        assert!(va.has_voted(51));
        assert!(va.has_voted(100));
    }

    #[test]
    fn test_vote_authority_signatures_are_valid() {
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());

        // Create votes for multiple slots and verify all signatures
        for slot in 1..=10 {
            let hash = Hash::new([slot as u8; 32]);
            let vote = va.try_vote(slot, hash).expect("First vote must succeed");
            assert!(
                vote.verify(),
                "Vote signature for slot {} must verify",
                slot
            );
        }
    }

    #[test]
    fn test_vote_authority_fork_scenario() {
        // Simulates the dangerous fork re-evaluation scenario:
        // 1. Producer creates block B for slot 5, votes via VoteAuthority
        // 2. Fork block B' for slot 5 arrives via P2P (different hash)
        // 3. VoteAuthority MUST refuse the second vote
        let kp = crate::Keypair::new();
        let mut va = VoteAuthority::new(kp.to_seed(), kp.pubkey());

        let producer_hash = Hash::new([0xAA; 32]);
        let fork_hash = Hash::new([0xBB; 32]);

        // Producer votes first
        let v1 = va.try_vote(5, producer_hash);
        assert!(v1.is_some(), "Producer's first vote must succeed");

        // Fork block arrives — VoteAuthority MUST refuse
        let v2 = va.try_vote(5, fork_hash);
        assert!(
            v2.is_none(),
            "Fork re-vote must be REFUSED to prevent DoubleVote slashing"
        );

        // Verify we recorded the original hash, not the fork
        assert!(va.has_voted(5));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TOKENOMICS OVERHAUL: Block Reward Constants
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_block_reward_constants() {
        // TX block reward: 0.02 MOLT = 20,000,000 shells
        assert_eq!(TRANSACTION_BLOCK_REWARD, 20_000_000);
        // Heartbeat block reward: 0.01 MOLT = 10,000,000 shells
        assert_eq!(HEARTBEAT_BLOCK_REWARD, 10_000_000);
        // BLOCK_REWARD alias = TRANSACTION_BLOCK_REWARD
        assert_eq!(BLOCK_REWARD, TRANSACTION_BLOCK_REWARD);
        // Heartbeat must be less than transaction reward
        assert!(HEARTBEAT_BLOCK_REWARD < TRANSACTION_BLOCK_REWARD);
        // Heartbeat is exactly 50% of transaction reward
        assert_eq!(HEARTBEAT_BLOCK_REWARD, TRANSACTION_BLOCK_REWARD / 2);
    }

    #[test]
    fn test_distribute_block_reward_values() {
        let mut pool = StakePool::new();
        let v1 = Pubkey::new([1u8; 32]);
        pool.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();

        // Transaction block reward should be 0.02 MOLT
        let tx_reward = pool.distribute_block_reward(&v1, 1, false);
        assert_eq!(
            tx_reward, 20_000_000,
            "TX block reward must be 0.02 MOLT (20M shells)"
        );

        // Heartbeat block reward should be 0.01 MOLT
        let hb_reward = pool.distribute_block_reward(&v1, 2, true);
        assert_eq!(
            hb_reward, 10_000_000,
            "Heartbeat reward must be 0.01 MOLT (10M shells)"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TOKENOMICS OVERHAUL: Reward Decay (20% Annual)
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_decayed_reward_year_0() {
        // Year 0 (slot 0 through SLOTS_PER_YEAR-1): no decay, full base reward
        assert_eq!(decayed_reward(20_000_000, 0), 20_000_000);
        assert_eq!(decayed_reward(10_000_000, 0), 10_000_000);
        // Just before the 1-year mark
        assert_eq!(decayed_reward(20_000_000, SLOTS_PER_YEAR - 1), 20_000_000);
    }

    #[test]
    fn test_decayed_reward_year_1() {
        // Year 1: 80% of base → 20M × 0.8 = 16M
        assert_eq!(decayed_reward(20_000_000, SLOTS_PER_YEAR), 16_000_000);
        assert_eq!(decayed_reward(10_000_000, SLOTS_PER_YEAR), 8_000_000);
        // Mid-year-1 (slot = 1.5 years → still year 1 since integer division)
        assert_eq!(
            decayed_reward(20_000_000, SLOTS_PER_YEAR + SLOTS_PER_YEAR / 2),
            16_000_000
        );
    }

    #[test]
    fn test_decayed_reward_year_5() {
        // Year 5: 0.8^5 = 0.32768 → 20M × 0.32768 = 6,553,600
        // Integer arithmetic: 20M × (80/100)^5
        let mut expected = 20_000_000u64;
        for _ in 0..5 {
            expected = expected * 80 / 100;
        }
        assert_eq!(decayed_reward(20_000_000, SLOTS_PER_YEAR * 5), expected);
        assert_eq!(expected, 6_553_600); // verify exact value

        // Heartbeat: 10M × 0.8^5 = 3,276,800
        let mut hb_expected = 10_000_000u64;
        for _ in 0..5 {
            hb_expected = hb_expected * 80 / 100;
        }
        assert_eq!(decayed_reward(10_000_000, SLOTS_PER_YEAR * 5), hb_expected);
        assert_eq!(hb_expected, 3_276_800);
    }

    #[test]
    fn test_decayed_reward_year_50() {
        // Year 50 (cap): 0.8^50 → effectively 0
        // After 50 years of 20% decay, 20M becomes ~285 shells
        let reward = decayed_reward(20_000_000, SLOTS_PER_YEAR * 50);
        assert!(
            reward < 500,
            "Year 50 reward should be near zero, got {}",
            reward
        );
        // Still > 0 due to integer rounding
        assert!(reward > 0, "Year 50 tx reward should not be exactly 0");

        // Heartbeat: even smaller
        let hb = decayed_reward(10_000_000, SLOTS_PER_YEAR * 50);
        assert!(
            hb < 250,
            "Year 50 heartbeat should be near zero, got {}",
            hb
        );
    }

    #[test]
    fn test_decayed_reward_overflow_safe() {
        // Extremely large slot values should not panic or overflow
        // AUDIT-FIX L2: reward decays to 0 past ~80 years, no cap needed
        let r100 = decayed_reward(20_000_000, SLOTS_PER_YEAR * 100);
        assert_eq!(r100, 0, "Year 100 reward should decay to 0");
        let r_max = decayed_reward(20_000_000, u64::MAX);
        assert_eq!(r_max, 0, "u64::MAX slot reward should decay to 0");
        // Zero base reward stays zero
        assert_eq!(decayed_reward(0, SLOTS_PER_YEAR * 10), 0);
        // u64::MAX base reward with year 0 — no decay applied, no overflow
        assert_eq!(decayed_reward(u64::MAX, 0), u64::MAX);
    }

    #[test]
    fn test_distribute_block_reward_with_decay() {
        // Verify distribute_block_reward applies decay at different slots
        let mut pool = StakePool::new();
        let v1 = Pubkey::new([1u8; 32]);
        pool.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();

        // Year 0: full reward
        let r0 = pool.distribute_block_reward(&v1, 100, false);
        assert_eq!(r0, 20_000_000, "Year 0 TX reward should be 0.02 MOLT");

        // Year 1: 80% reward
        let r1 = pool.distribute_block_reward(&v1, SLOTS_PER_YEAR, false);
        assert_eq!(r1, 16_000_000, "Year 1 TX reward should be 0.016 MOLT");

        // Year 1 heartbeat: 80% of 10M = 8M
        let h1 = pool.distribute_block_reward(&v1, SLOTS_PER_YEAR + 1, true);
        assert_eq!(h1, 8_000_000, "Year 1 heartbeat should be 0.008 MOLT");
    }

    #[test]
    fn test_annual_reward_decay_constant() {
        assert_eq!(
            ANNUAL_REWARD_DECAY_BPS, 2000,
            "Decay must be 20% (2000 bps)"
        );
    }

    #[test]
    fn test_fee_share_goes_through_vesting() {
        // Producer with bootstrap_debt should receive only ~50% of fee share as
        // liquid, with the rest repaying debt (same vesting pipeline as block rewards).
        let mut pool = StakePool::new();
        let v1 = Pubkey::new([1u8; 32]);
        // Use bootstrap index 0 + BOOTSTRAP_GRANT_AMOUNT to create validator with debt
        pool.stake_with_index(v1, BOOTSTRAP_GRANT_AMOUNT, 0, 0)
            .unwrap();

        // Confirm producer has bootstrap debt
        let info = pool.get_stake(&v1).unwrap();
        assert!(
            info.bootstrap_debt > 0,
            "Bootstrap validator must have bootstrap debt"
        );
        assert_eq!(info.bootstrap_debt, BOOTSTRAP_GRANT_AMOUNT);

        // Distribute fee reward (simulates 30% producer share)
        let fee_share = 300_000; // small fee amount in shells
        pool.distribute_fees(&v1, fee_share, 100);

        // Claim should produce a vesting split: ~50% liquid, ~50% debt repayment
        let (liquid, debt_payment) = pool.claim_rewards(&v1, 100);
        assert_eq!(
            liquid + debt_payment,
            fee_share,
            "liquid + debt_payment must equal full fee share"
        );
        assert!(
            liquid > 0 && liquid < fee_share,
            "With debt, liquid ({}) must be between 0 and full share ({})",
            liquid,
            fee_share
        );
        assert!(
            debt_payment > 0,
            "With debt, some portion must go to debt repayment"
        );
        // Standard 50/50 split: liquid = fee_share - (fee_share/2).min(debt)
        // fee_share/2 = 150_000; debt >> 150_000, so paid = 150_000, liquid = 150_000
        assert_eq!(
            liquid, 150_000,
            "Standard 50/50 split: liquid should be half"
        );
        assert_eq!(
            debt_payment, 150_000,
            "Standard 50/50 split: debt payment should be half"
        );
    }

    #[test]
    fn test_fee_share_fully_vested() {
        // Producer with no bootstrap_debt should receive 100% of fee share as liquid.
        let mut pool = StakePool::new();
        let v1 = Pubkey::new([1u8; 32]);
        // Self-funded validator (index u64::MAX) gets no bootstrap debt
        pool.stake(v1, MIN_VALIDATOR_STAKE, 0).unwrap();

        // Verify no debt
        let info = pool.get_stake(&v1).unwrap();
        assert_eq!(
            info.bootstrap_debt, 0,
            "Self-funded validator should have no debt"
        );

        let fee_share = 500_000;
        pool.distribute_fees(&v1, fee_share, 200);
        let (liquid, debt_payment) = pool.claim_rewards(&v1, 200);

        assert_eq!(
            liquid, fee_share,
            "Fully vested: 100% of fee share should be liquid"
        );
        assert_eq!(debt_payment, 0, "Fully vested: no debt repayment");
    }

    // ============================================================
    // Founding moltys vesting tests
    // ============================================================

    #[test]
    fn test_founding_moltys_locked_at_genesis() {
        // At genesis (time 0), no tokens should be unlocked.
        let total = 100_000_000 * 1_000_000_000u64; // 100M MOLT in shells
        let genesis_time = 1_700_000_000u64; // arbitrary genesis timestamp
        let cliff_end = genesis_time + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_time + FOUNDING_VEST_TOTAL_SECONDS;

        // Right at genesis: 0 unlocked
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, genesis_time);
        assert_eq!(
            unlocked, 0,
            "At genesis, no founding moltys should be unlocked"
        );

        // 1 second after genesis: still 0 (within cliff)
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, genesis_time + 1);
        assert_eq!(unlocked, 0, "During cliff period, no tokens unlock");
    }

    #[test]
    fn test_founding_moltys_cliff_not_reached() {
        // No tokens unlock before the 6-month cliff ends.
        let total = 100_000_000 * 1_000_000_000u64;
        let genesis_time = 1_700_000_000u64;
        let cliff_end = genesis_time + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_time + FOUNDING_VEST_TOTAL_SECONDS;

        // 1 second before cliff ends: still 0
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, cliff_end - 1);
        assert_eq!(unlocked, 0, "1 second before cliff: no tokens unlock");

        // Halfway through cliff: still 0
        let halfway = genesis_time + FOUNDING_CLIFF_SECONDS / 2;
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, halfway);
        assert_eq!(unlocked, 0, "Halfway through cliff: no tokens unlock");
    }

    #[test]
    fn test_founding_moltys_partial_vest() {
        // After cliff, tokens unlock linearly over 18 months.
        let total = 100_000_000 * 1_000_000_000u64; // 100M MOLT
        let genesis_time = 1_700_000_000u64;
        let cliff_end = genesis_time + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_time + FOUNDING_VEST_TOTAL_SECONDS;

        // Right at cliff end: 0% of linear period elapsed → 0 unlocked
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, cliff_end);
        assert_eq!(
            unlocked, 0,
            "At cliff end, linear period hasn't started yielding"
        );

        // 1 second after cliff: tiny amount unlocked
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, cliff_end + 1);
        assert!(
            unlocked > 0,
            "1 second after cliff, some tokens should unlock"
        );
        assert!(
            unlocked < total / 1_000,
            "1 second after cliff: unlocked should be tiny"
        );

        // Halfway through linear period (9 months after cliff = 15 months total)
        let linear_period = vest_end - cliff_end; // 18 months in seconds
        let halfway_linear = cliff_end + linear_period / 2;
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, halfway_linear);
        // Should be approximately 50% of total
        let expected_half = total / 2;
        let tolerance = total / 1000; // 0.1% tolerance for integer rounding
        assert!(
            unlocked >= expected_half - tolerance && unlocked <= expected_half + tolerance,
            "Halfway through linear vest: expected ~{} but got {}",
            expected_half,
            unlocked
        );

        // 3/4 through linear period (13.5 months after cliff)
        let three_quarter = cliff_end + linear_period * 3 / 4;
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, three_quarter);
        let expected_75 = total * 3 / 4;
        assert!(
            unlocked >= expected_75 - tolerance && unlocked <= expected_75 + tolerance,
            "75% through linear vest: expected ~{} but got {}",
            expected_75,
            unlocked
        );
    }

    #[test]
    fn test_founding_moltys_fully_vested() {
        // After 24 months total, 100% should be unlocked.
        let total = 100_000_000 * 1_000_000_000u64;
        let genesis_time = 1_700_000_000u64;
        let cliff_end = genesis_time + FOUNDING_CLIFF_SECONDS;
        let vest_end = genesis_time + FOUNDING_VEST_TOTAL_SECONDS;

        // Exactly at vest_end: fully vested
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, vest_end);
        assert_eq!(unlocked, total, "At vest end, 100% should be unlocked");

        // Well past vest_end (10 years later)
        let unlocked =
            founding_vesting_unlocked(total, cliff_end, vest_end, vest_end + 10 * 365 * 86400);
        assert_eq!(unlocked, total, "Long after vest end, still 100%");

        // 1 second before vest_end: almost fully vested but not quite
        let unlocked = founding_vesting_unlocked(total, cliff_end, vest_end, vest_end - 1);
        assert!(
            unlocked < total,
            "1 second before vest end: should be slightly less than total"
        );
        assert!(
            unlocked > total * 99 / 100,
            "1 second before vest end: should be >99% vested"
        );
    }
}
