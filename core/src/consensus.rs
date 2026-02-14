// MoltChain Consensus Module
// Byzantine Fault Tolerant consensus with Proof of Contribution

use crate::contract::ContractAccount;
use crate::{Hash, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

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
pub fn read_molt_price_feed_from_state(
    state: &crate::state::StateStore,
) -> Option<(u64, u8, u64)> {
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
            // Check staleness: reject if > 1 hour old
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
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
            if price < 0.000001 || price > 1_000_000.0 {
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
        // Only bootstrap if this is one of the first 200 AND amount matches MIN_VALIDATOR_STAKE
        let is_bootstrap = bootstrap_index < MAX_BOOTSTRAP_VALIDATORS
            && amount == MIN_VALIDATOR_STAKE;
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
                // All reward is liquid after time-cap graduation
                self.total_claimed = self.total_claimed.saturating_add(total_reward);
                return (total_reward, 0);
            }

            // ── Performance bonus: 95%+ uptime → accelerated repayment ──
            // PERFORMANCE_BONUS_BPS = 15000 → 1.5× multiplier on the 50% debt portion.
            // Effective split: debt = 50% × 1.5 = 75%, liquid = 25%.
            let debt_fraction = if self.uptime_bps(current_slot, num_validators) >= UPTIME_BONUS_THRESHOLD_BPS {
                // Accelerated: debt_portion = base_50% × (PERFORMANCE_BONUS_BPS / 10000)
                let base_half = total_reward / 2;
                (base_half as u128 * PERFORMANCE_BONUS_BPS as u128 / 10000) as u64
            } else {
                // Standard 50% to debt repayment
                total_reward / 2
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

/// Custom serde for HashMap<[u8; 32], Pubkey> — JSON requires string keys.
/// Keys are hex-encoded for serialization, decoded on deserialization.
mod fingerprint_serde {
    use super::*;
    use serde::de::{self, Deserializer, MapAccess, Visitor};
    use serde::ser::{SerializeMap, Serializer};
    use std::fmt;

    pub fn serialize<S>(
        map: &HashMap<[u8; 32], Pubkey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut ser_map = serializer.serialize_map(Some(map.len()))?;
        for (key, value) in map {
            ser_map.serialize_entry(&hex::encode(key), value)?;
        }
        ser_map.end()
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashMap<[u8; 32], Pubkey>, D::Error>
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
    #[serde(default, serialize_with = "fingerprint_serde::serialize", deserialize_with = "fingerprint_serde::deserialize")]
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
    ///
    /// Automatically computes the active validator count from the stake pool
    /// and passes it to StakeInfo::claim_rewards for correct uptime calculation.
    pub fn claim_rewards(&mut self, validator: &Pubkey, current_slot: u64) -> (u64, u64) {
        // Count active validators for uptime formula:
        // expected_blocks = slots_active / num_active_validators
        let num_active: u64 = self
            .stakes
            .values()
            .filter(|info| info.is_active)
            .count() as u64;
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

        if let Some(stake_info) = self.stakes.get_mut(&validator) {
            stake_info.amount += amount;
            stake_info.is_active = stake_info.meets_minimum();
        } else {
            let stake_info =
                StakeInfo::with_bootstrap_index(validator, amount, current_slot, bootstrap_index);
            self.stakes.insert(validator, stake_info);
        }

        self.total_staked += amount;
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
        self.fingerprint_registry.insert(new_fingerprint, *validator);

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
        // If validator already exists, just update amount (no new bootstrap)
        if self.stakes.contains_key(&validator) {
            let stake_info = self.stakes.get_mut(&validator).unwrap();
            stake_info.amount += amount;
            stake_info.is_active = stake_info.meets_minimum();
            let existing_index = stake_info.bootstrap_index;
            self.total_staked += amount;
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
            return Err(format!("Stake {} is below minimum {}", amount, MIN_VALIDATOR_STAKE));
        }

        let stake_info = StakeInfo::with_bootstrap_index(validator, amount, current_slot, bootstrap_index);
        self.stakes.insert(validator, stake_info);
        self.total_staked += amount;

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
            pool.stake_with_index(pk, MIN_VALIDATOR_STAKE, 0, idx)
                .unwrap();

            // Confirm bootstrap debt exists
            let stake = pool.get_stake(&pk).unwrap();
            assert_eq!(stake.bootstrap_debt, MIN_VALIDATOR_STAKE);
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
        let mut stake = StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, 0, 0);

        assert_eq!(stake.bootstrap_debt, MIN_VALIDATOR_STAKE);
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
        assert_eq!(stake.bootstrap_debt, MIN_VALIDATOR_STAKE - 500_000_000);
        assert_eq!(stake.earned_amount, 500_000_000);
        assert_eq!(stake.total_claimed, 1_000_000_000);
        assert_eq!(stake.total_debt_repaid, 500_000_000);
    }

    #[test]
    fn test_performance_bonus_75_25_split() {
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, 0, 0);

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
        let mut stake = StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, 0, 0);

        assert_eq!(stake.status, BootstrapStatus::Bootstrapping);
        assert_eq!(stake.bootstrap_debt, MIN_VALIDATOR_STAKE);

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
        let mut stake =
            StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, start_slot, 0);

        // Add a small reward — nowhere near enough to repay debt
        stake.add_reward(1_000_000_000, start_slot + 100); // 1 MOLT

        // Claim before time cap — normal 50/50 split (0 blocks → 0 uptime)
        let (liquid1, debt1) = stake.claim_rewards(start_slot + 100, 1);
        assert_eq!(liquid1, 500_000_000);
        assert_eq!(debt1, 500_000_000);
        assert_eq!(stake.status, BootstrapStatus::Bootstrapping);

        // Now add another small reward and claim PAST the time cap
        stake.add_reward(2_000_000_000, start_slot + MAX_BOOTSTRAP_SLOTS + 1);
        let (liquid2, debt2) =
            stake.claim_rewards(start_slot + MAX_BOOTSTRAP_SLOTS + 1, 1);

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

        pool.stake_with_index(pk1, MIN_VALIDATOR_STAKE, 0, 0).unwrap();
        pool.stake_with_index(pk2, MIN_VALIDATOR_STAKE, 0, 1).unwrap();

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

        pool.stake_with_index(pk1, MIN_VALIDATOR_STAKE, 0, 0).unwrap();
        pool.stake_with_index(pk2, MIN_VALIDATOR_STAKE, 0, 1).unwrap();

        let zero_fp = [0u8; 32];

        // Zero fingerprints should always be accepted (dev mode / legacy)
        assert!(pool.register_fingerprint(&pk1, zero_fp).is_ok());
        assert!(pool.register_fingerprint(&pk2, zero_fp).is_ok());
    }

    #[test]
    fn test_machine_migration() {
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        pool.stake_with_index(pk, MIN_VALIDATOR_STAKE, 0, 0).unwrap();

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
        let mut stake = StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, 0, 0);

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
        assert!(uptime_95 >= 9500, "uptime {} should be >= 9500 at 95%", uptime_95);

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
            pool.stake_with_index(dummy, MIN_VALIDATOR_STAKE, 0, idx)
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
            pool.stake_with_index(pk, MIN_VALIDATOR_STAKE, 0, idx).unwrap();
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
        pool.stake_with_index(pk_with_fp, MIN_VALIDATOR_STAKE, 0, next_idx).unwrap();
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
        pool.stake_with_index(pk1, MIN_VALIDATOR_STAKE, 0, 0).unwrap();
        pool.register_fingerprint(&pk1, shared_fp).unwrap();

        // pk2 is self-funded (index u64::MAX) — same fingerprint
        pool.stake_with_index(pk2, MIN_VALIDATOR_STAKE, 0, u64::MAX).unwrap();
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
            .try_bootstrap_with_fingerprint(pk1, MIN_VALIDATOR_STAKE, 0, fingerprint)
            .unwrap();
        assert_eq!(idx1, 0);
        assert!(is_new1);
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // pk2 tries same fingerprint → should fail
        let err = pool
            .try_bootstrap_with_fingerprint(pk2, MIN_VALIDATOR_STAKE, 0, fingerprint)
            .unwrap_err();
        assert!(err.contains("already registered"));

        // Counter should still be 1 (NOT 2) — index was NOT wasted
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // pk2 with a different fingerprint succeeds
        let fp2 = [0xCC; 32];
        let (idx2, is_new2) = pool
            .try_bootstrap_with_fingerprint(pk2, MIN_VALIDATOR_STAKE, 0, fp2)
            .unwrap();
        assert_eq!(idx2, 1);
        assert!(is_new2);
        assert_eq!(pool.bootstrap_grants_issued(), 2);
    }

    #[test]
    fn test_try_bootstrap_existing_validator_restake() {
        // Existing validator re-staking doesn't get a new bootstrap index
        let mut pool = StakePool::new();
        let pk = Pubkey::new([1u8; 32]);
        let fp = [0xDD; 32];

        // First bootstrap
        let (idx, is_new) = pool
            .try_bootstrap_with_fingerprint(pk, MIN_VALIDATOR_STAKE, 0, fp)
            .unwrap();
        assert_eq!(idx, 0);
        assert!(is_new);

        // Re-stake same validator
        let (idx2, is_new2) = pool
            .try_bootstrap_with_fingerprint(pk, MIN_VALIDATOR_STAKE, 100, fp)
            .unwrap();
        assert_eq!(idx2, 0); // Same index
        assert!(!is_new2); // Not new

        // Counter should be 1 (only one grant)
        assert_eq!(pool.bootstrap_grants_issued(), 1);

        // Total stake doubled
        let stake = pool.get_stake(&pk).unwrap();
        assert_eq!(stake.amount, MIN_VALIDATOR_STAKE * 2);
    }

    #[test]
    fn test_performance_bonus_uses_constant() {
        // Verify PERFORMANCE_BONUS_BPS = 15000 produces 75% debt fraction
        // base_half = reward / 2, debt = base_half * 15000 / 10000 = base_half * 1.5
        let pk = Pubkey::new([1u8; 32]);
        let mut stake = StakeInfo::with_bootstrap_index(pk, MIN_VALIDATOR_STAKE, 0, 0);
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
}
