// Lichen MossStake - Liquid Staking Protocol
// Stake LICN, receive stLICN (liquid receipt token)

use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::consensus::UNSTAKE_COOLDOWN_SLOTS;
use crate::Pubkey;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Percentage of epoch inflation allocated to MossStake stakers (basis points).
/// The constant name is preserved for compatibility with existing callers.
/// 1000 bp = 10% of the settled epoch mint funds the liquid staking pool.
pub const MOSSSTAKE_BLOCK_SHARE_BPS: u64 = 1_000;
pub const MOSSSTAKE_SLOT_ONLY_METADATA_KEY: &str = "mossstake_slot_only_v1";
pub const LEGACY_TESTNET_MOSSSTAKE_WALL_CLOCK_START_PARENT_SLOT: u64 = 2_384_143;
pub const LEGACY_TESTNET_MOSSSTAKE_SLOT_ONLY_ACTIVATION_PARENT_SLOT: u64 = 2_710_766;
const SECONDS_PER_DAY: u64 = 86_400;
pub const MOSSSTAKE_UNSTAKE_COOLDOWN_SECONDS: u64 = 7 * SECONDS_PER_DAY;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MossStakeReplayMode {
    SlotOnly,
    LegacyWallClock,
}

impl MossStakeReplayMode {
    pub fn uses_wall_clock(self) -> bool {
        matches!(self, Self::LegacyWallClock)
    }
}

/// Serde helper: serialize/deserialize HashMap<Pubkey, V> with base58 string keys.
/// JSON requires map keys to be strings; Pubkey normally serializes as [u8;32].
mod pubkey_map_serde {
    use super::*;
    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeMap;

    pub fn serialize<V: Serialize, S: serde::Serializer>(
        map: &HashMap<Pubkey, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut m = serializer.serialize_map(Some(map.len()))?;
        for (k, v) in map {
            m.serialize_entry(&k.to_base58(), v)?;
        }
        m.end()
    }

    pub fn deserialize<'de, V: Deserialize<'de>, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<HashMap<Pubkey, V>, D::Error> {
        struct PubkeyMapVisitor<V>(std::marker::PhantomData<V>);

        impl<'de, V: Deserialize<'de>> Visitor<'de> for PubkeyMapVisitor<V> {
            type Value = HashMap<Pubkey, V>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map with base58 pubkey string keys")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
                let mut map = HashMap::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    let pubkey = Pubkey::from_base58(&key).map_err(de::Error::custom)?;
                    map.insert(pubkey, value);
                }
                Ok(map)
            }
        }

        deserializer.deserialize_map(PubkeyMapVisitor(std::marker::PhantomData))
    }
}

/// Serde helper: serialize/deserialize BTreeMap<Pubkey, V> with base58 string keys.
/// Deterministic iteration order (sorted by Pubkey bytes) is critical for consensus.
mod pubkey_btreemap_serde {
    use super::*;
    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeMap;

    pub fn serialize<V: Serialize, S: serde::Serializer>(
        map: &BTreeMap<Pubkey, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut m = serializer.serialize_map(Some(map.len()))?;
        for (k, v) in map {
            m.serialize_entry(&k.to_base58(), v)?;
        }
        m.end()
    }

    pub fn deserialize<'de, V: Deserialize<'de>, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<Pubkey, V>, D::Error> {
        struct PubkeyBTreeMapVisitor<V>(std::marker::PhantomData<V>);

        impl<'de, V: Deserialize<'de>> Visitor<'de> for PubkeyBTreeMapVisitor<V> {
            type Value = BTreeMap<Pubkey, V>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map with base58 pubkey string keys")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
                let mut map = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    let pubkey = Pubkey::from_base58(&key).map_err(de::Error::custom)?;
                    map.insert(pubkey, value);
                }
                Ok(map)
            }
        }

        deserializer.deserialize_map(PubkeyBTreeMapVisitor(std::marker::PhantomData))
    }
}

/// stLICN token - liquid staking receipt
/// T3.2/T6.2 fix: All math is integer-only (fixed-point with PRECISION denominator).
/// No floating-point is used anywhere in consensus-critical code.
///
/// Exchange rate is stored as basis points: rate_bp = (total_licn * RATE_PRECISION) / total_supply
/// RATE_PRECISION = 1_000_000_000 (1e9) to match spore precision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StLicnToken {
    pub total_supply: u64,      // Total stLICN in circulation
    pub total_licn_staked: u64, // Total LICN backing stLICN
    /// Exchange rate in fixed-point: (LICN_per_stLICN * RATE_PRECISION)
    /// e.g., 1_000_000_000 = 1.0x, 1_100_000_000 = 1.1x
    pub exchange_rate_fp: u64,
}

/// Fixed-point precision for exchange rate (1e9)
const RATE_PRECISION: u128 = 1_000_000_000;

impl Default for StLicnToken {
    fn default() -> Self {
        Self::new()
    }
}

impl StLicnToken {
    pub fn new() -> Self {
        Self {
            total_supply: 0,
            total_licn_staked: 0,
            exchange_rate_fp: RATE_PRECISION as u64, // 1.0 initially
        }
    }

    /// Calculate exchange rate as fixed-point (LICN per stLICN * RATE_PRECISION)
    /// Increases as rewards accumulate.
    pub fn calculate_exchange_rate_fp(&self) -> u64 {
        if self.total_supply == 0 {
            RATE_PRECISION as u64
        } else {
            // Use u128 to avoid overflow: (total_licn * PRECISION) / total_supply
            ((self.total_licn_staked as u128 * RATE_PRECISION) / self.total_supply as u128) as u64
        }
    }

    /// Calculate exchange rate as f64 (for display/API only — NOT for consensus math)
    pub fn exchange_rate_display(&self) -> f64 {
        self.exchange_rate_fp as f64 / RATE_PRECISION as f64
    }

    /// Calculate stLICN to mint for given LICN amount (integer math only)
    pub fn licn_to_st_licn(&self, licn_amount: u64) -> u64 {
        if self.total_supply == 0 {
            licn_amount
        } else {
            // st_licn_amount_out = (licn * PRECISION) / exchange_rate_fp
            let rate = self.exchange_rate_fp.max(1) as u128;
            ((licn_amount as u128 * RATE_PRECISION) / rate) as u64
        }
    }

    /// Calculate LICN to return for given stLICN amount (integer math only)
    pub fn st_licn_to_licn(&self, st_licn_amount: u64) -> u64 {
        // licn = (st_licn_amount_out * exchange_rate_fp) / PRECISION
        ((st_licn_amount as u128 * self.exchange_rate_fp as u128) / RATE_PRECISION) as u64
    }
}

/// Staking lock tier — determines APY bonus and lock duration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LockTier {
    #[default]
    Flexible = 0, // No lock, 7-day unstake cooldown, 1.0x multiplier
    Lock30 = 1,  // 30-day lock, 1.1x multiplier
    Lock180 = 2, // 180-day lock, 1.25x multiplier
    Lock365 = 3, // 365-day lock, 1.5x multiplier
}

impl LockTier {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Flexible),
            1 => Some(Self::Lock30),
            2 => Some(Self::Lock180),
            3 => Some(Self::Lock365),
            _ => None,
        }
    }

    /// Reward multiplier in basis points (10000 = 1.0x)
    /// Target APY ratios: Flexible ~5%, 30-Day ~8%, 180-Day ~12%, 365-Day ~18%
    pub fn reward_multiplier_bp(&self) -> u64 {
        match self {
            Self::Flexible => 10_000, // 1.0x  (base ~5%)
            Self::Lock30 => 16_000,   // 1.6x  (~8%)
            Self::Lock180 => 24_000,  // 2.4x  (~12%)
            Self::Lock365 => 36_000,  // 3.6x  (~18%)
        }
    }

    /// Lock duration in slots. MossStake consensus is slot-only; changing the
    /// wall-clock cadence does not mutate lock or cooldown accounting.
    pub fn lock_duration_slots(&self) -> u64 {
        match self {
            Self::Flexible => 0,       // No lock (7-day unstake cooldown applies separately)
            Self::Lock30 => 6_480_000, // 30 days
            Self::Lock180 => 38_880_000, // 180 days
            Self::Lock365 => 78_840_000, // 365 days
        }
    }

    /// Legacy v0.5.93 wall-clock duration. Only historical replay of the
    /// deployed v0.5.93 testnet interval uses this value.
    pub fn lock_duration_seconds(&self) -> u64 {
        match self {
            Self::Flexible => 0,
            Self::Lock30 => 30 * SECONDS_PER_DAY,
            Self::Lock180 => 180 * SECONDS_PER_DAY,
            Self::Lock365 => 365 * SECONDS_PER_DAY,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Flexible => "Flexible",
            Self::Lock30 => "30-Day Lock",
            Self::Lock180 => "180-Day Lock",
            Self::Lock365 => "365-Day Lock",
        }
    }
}

/// User's staking position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingPosition {
    pub owner: Pubkey,
    pub st_licn_amount: u64, // stLICN balance
    pub licn_deposited: u64, // Original LICN deposited
    pub deposited_at: u64,   // Slot when deposited
    // Legacy v0.5.93 wall-clock field. It is decoded for old RocksDB rows only;
    // runtime and canonical hashing are slot-only.
    #[serde(default)]
    pub deposited_at_unix_seconds: u64,
    pub rewards_earned: u64, // Accumulated rewards (auto-compound)
    #[serde(default)]
    pub lock_tier: LockTier, // Staking tier (Flexible, 30d, 90d, 365d)
    #[serde(default)]
    pub lock_until: u64, // Slot lock deadline (0 = no lock)
    // Legacy v0.5.93 wall-clock field. It is decoded for old RocksDB rows only;
    // runtime and canonical hashing are slot-only.
    #[serde(default)]
    pub lock_until_unix_seconds: u64,
}

/// Unstaking request (7-day cooldown)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnstakeRequest {
    pub owner: Pubkey,
    pub st_licn_amount: u64,  // stLICN being unstaked
    pub licn_to_receive: u64, // LICN to receive (locked rate)
    pub requested_at: u64,    // Slot when requested
    pub claimable_at: u64,    // Slot when can claim (requested + 7 days)
    // Legacy v0.5.93 wall-clock fields. They are decoded for old RocksDB rows
    // only; runtime and canonical hashing are slot-only.
    #[serde(default)]
    pub requested_at_unix_seconds: u64,
    #[serde(default)]
    pub claimable_at_unix_seconds: u64,
}

/// MossStake liquid staking pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MossStakePool {
    pub st_licn_token: StLicnToken,
    #[serde(with = "pubkey_btreemap_serde")]
    pub positions: BTreeMap<Pubkey, StakingPosition>,
    #[serde(with = "pubkey_map_serde")]
    pub unstake_requests: HashMap<Pubkey, Vec<UnstakeRequest>>,
    pub total_validators: u64, // Number of validators staked to
    /// Average APY in basis points (10000 = 100.00%)
    pub average_apy_bp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MossStakePoolSnapshotV1 {
    version: u8,
    st_licn_token: StLicnToken,
    positions: Vec<(Pubkey, StakingPosition)>,
    unstake_requests: Vec<(Pubkey, Vec<UnstakeRequest>)>,
    total_validators: u64,
    average_apy_bp: u64,
}

impl Default for MossStakePool {
    fn default() -> Self {
        Self::new()
    }
}

impl MossStakePool {
    pub fn new() -> Self {
        Self {
            st_licn_token: StLicnToken::new(),
            positions: BTreeMap::new(),
            unstake_requests: HashMap::new(),
            total_validators: 0,
            average_apy_bp: 0,
        }
    }

    /// Current redeemable value for a position.
    ///
    /// Rewards are distributed into each position according to its tier-weighted
    /// shares, so redemption must use the position accounting rather than the
    /// pool-wide average exchange rate. The global exchange rate remains useful
    /// for minting new shares at the current average pool price.
    pub fn position_value(&self, position: &StakingPosition) -> u64 {
        position
            .licn_deposited
            .saturating_add(position.rewards_earned)
    }

    fn split_position_value_for_unstake(
        position: &StakingPosition,
        st_licn_amount: u64,
    ) -> Result<(u64, u64, u64), String> {
        if position.st_licn_amount == 0 {
            return Err("Position has no stLICN".to_string());
        }
        if position.st_licn_amount < st_licn_amount {
            return Err(format!(
                "Insufficient stLICN: have {}, need {}",
                position.st_licn_amount, st_licn_amount
            ));
        }

        if st_licn_amount == position.st_licn_amount {
            let licn_to_receive = position
                .licn_deposited
                .saturating_add(position.rewards_earned);
            return Ok((
                position.licn_deposited,
                position.rewards_earned,
                licn_to_receive,
            ));
        }

        let principal_out = ((st_licn_amount as u128 * position.licn_deposited as u128)
            / position.st_licn_amount as u128) as u64;
        let rewards_out = ((st_licn_amount as u128 * position.rewards_earned as u128)
            / position.st_licn_amount as u128) as u64;
        Ok((
            principal_out,
            rewards_out,
            principal_out.saturating_add(rewards_out),
        ))
    }

    /// Clear legacy v0.5.93 wall-clock fields from decoded pools.
    ///
    /// This keeps old RocksDB data readable while returning persisted MossStake
    /// state to slot-only semantics.
    pub fn clear_wall_clock_times(&mut self) -> bool {
        let mut changed = false;

        for position in self.positions.values_mut() {
            if position.deposited_at_unix_seconds != 0 {
                position.deposited_at_unix_seconds = 0;
                changed = true;
            }
            if position.lock_until_unix_seconds != 0 {
                position.lock_until_unix_seconds = 0;
                changed = true;
            }
        }

        for requests in self.unstake_requests.values_mut() {
            for request in requests {
                if request.requested_at_unix_seconds != 0 {
                    request.requested_at_unix_seconds = 0;
                    changed = true;
                }
                if request.claimable_at_unix_seconds != 0 {
                    request.claimable_at_unix_seconds = 0;
                    changed = true;
                }
            }
        }

        changed
    }

    pub fn legacy_slot_timestamp(slot: u64) -> u64 {
        slot.saturating_mul(400) / 1_000
    }

    pub fn backfill_wall_clock_times<F>(&mut self, mut timestamp_for_slot: F) -> bool
    where
        F: FnMut(u64) -> Option<u64>,
    {
        let mut changed = false;

        for position in self.positions.values_mut() {
            if position.deposited_at_unix_seconds == 0 {
                if let Some(ts) = timestamp_for_slot(position.deposited_at) {
                    position.deposited_at_unix_seconds = ts;
                    changed = true;
                }
            }
            if position.lock_until_unix_seconds == 0 && position.lock_until > 0 {
                let deposit_ts = if position.deposited_at_unix_seconds > 0 {
                    Some(position.deposited_at_unix_seconds)
                } else {
                    timestamp_for_slot(position.deposited_at)
                };
                if let Some(ts) = deposit_ts {
                    position.lock_until_unix_seconds =
                        ts.saturating_add(position.lock_tier.lock_duration_seconds());
                    changed = true;
                }
            }
        }

        for requests in self.unstake_requests.values_mut() {
            for request in requests {
                if request.requested_at_unix_seconds == 0 {
                    if let Some(ts) = timestamp_for_slot(request.requested_at) {
                        request.requested_at_unix_seconds = ts;
                        changed = true;
                    }
                }
                if request.claimable_at_unix_seconds == 0 {
                    let request_ts = if request.requested_at_unix_seconds > 0 {
                        Some(request.requested_at_unix_seconds)
                    } else {
                        timestamp_for_slot(request.requested_at)
                    };
                    if let Some(ts) = request_ts {
                        request.claimable_at_unix_seconds =
                            ts.saturating_add(MOSSSTAKE_UNSTAKE_COOLDOWN_SECONDS);
                        changed = true;
                    }
                }
            }
        }

        changed
    }

    fn assert_transferable_position(
        position: &StakingPosition,
        current_slot: u64,
    ) -> Result<(), String> {
        if position.lock_tier != LockTier::Flexible {
            return Err(format!(
                "{} positions are not transferable. Unstake after the lock expires to receive LICN, or use the Flexible tier for liquid stLICN.",
                position.lock_tier.display_name()
            ));
        }
        if position.lock_until > current_slot {
            return Err(format!(
                "Position locked until slot {}; locked MossStake positions are not transferable",
                position.lock_until
            ));
        }
        Ok(())
    }

    fn assert_transferable_position_at_time(
        position: &StakingPosition,
        current_slot: u64,
        current_unix_seconds: u64,
    ) -> Result<(), String> {
        if position.lock_tier != LockTier::Flexible {
            return Err(format!(
                "{} positions are not transferable. Unstake after the lock expires to receive LICN, or use the Flexible tier for liquid stLICN.",
                position.lock_tier.display_name()
            ));
        }
        if position.lock_until_unix_seconds > current_unix_seconds {
            return Err(format!(
                "Position locked until timestamp {}; locked MossStake positions are not transferable",
                position.lock_until_unix_seconds
            ));
        }
        if position.lock_until > current_slot {
            return Err(format!(
                "Position locked until slot {}; locked MossStake positions are not transferable",
                position.lock_until
            ));
        }
        Ok(())
    }

    fn total_weighted_st_licn(&self) -> u128 {
        self.positions
            .values()
            .map(|p| {
                (p.st_licn_amount as u128 * p.lock_tier.reward_multiplier_bp() as u128) / 10_000
            })
            .sum()
    }

    /// Stake LICN, mint stLICN
    pub fn stake(
        &mut self,
        user: Pubkey,
        licn_amount: u64,
        current_slot: u64,
    ) -> Result<u64, String> {
        self.stake_with_tier(user, licn_amount, current_slot, LockTier::Flexible)
    }

    /// Stake LICN with a specific lock tier
    pub fn stake_with_tier(
        &mut self,
        user: Pubkey,
        licn_amount: u64,
        current_slot: u64,
        tier: LockTier,
    ) -> Result<u64, String> {
        if licn_amount == 0 {
            return Err("Cannot stake 0 LICN".to_string());
        }

        // Calculate stLICN to mint
        let st_licn_to_mint = self.st_licn_token.licn_to_st_licn(licn_amount);

        // Update pool
        self.st_licn_token.total_supply += st_licn_to_mint;
        self.st_licn_token.total_licn_staked += licn_amount;
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        // Calculate lock expiry
        let lock_until = if tier.lock_duration_slots() > 0 {
            current_slot + tier.lock_duration_slots()
        } else {
            0
        };

        // Update user position
        if let Some(position) = self.positions.get_mut(&user) {
            // AUDIT-FIX D-4: Reject tier changes on existing positions to prevent
            // silently locking all previous deposits under a longer lock period.
            // Users must withdraw and re-stake to change tiers.
            if tier != position.lock_tier {
                return Err(format!(
                    "Cannot change lock tier on existing position (current: {:?}, requested: {:?}). Withdraw first.",
                    position.lock_tier, tier
                ));
            }
            position.st_licn_amount += st_licn_to_mint;
            position.licn_deposited += licn_amount;
            // Extend lock to the later of current lock or new deposit lock
            // This prevents the exploit of depositing large amounts right before unlock
            if lock_until > position.lock_until {
                position.lock_until = lock_until;
            }
            position.deposited_at_unix_seconds = 0;
            position.lock_until_unix_seconds = 0;
        } else {
            self.positions.insert(
                user,
                StakingPosition {
                    owner: user,
                    st_licn_amount: st_licn_to_mint,
                    licn_deposited: licn_amount,
                    deposited_at: current_slot,
                    deposited_at_unix_seconds: 0,
                    rewards_earned: 0,
                    lock_tier: tier,
                    lock_until,
                    lock_until_unix_seconds: 0,
                },
            );
        }

        Ok(st_licn_to_mint)
    }

    pub fn stake_with_tier_at_time(
        &mut self,
        user: Pubkey,
        licn_amount: u64,
        current_slot: u64,
        current_unix_seconds: u64,
        tier: LockTier,
    ) -> Result<u64, String> {
        if licn_amount == 0 {
            return Err("Cannot stake 0 LICN".to_string());
        }

        let st_licn_to_mint = self.st_licn_token.licn_to_st_licn(licn_amount);

        self.st_licn_token.total_supply += st_licn_to_mint;
        self.st_licn_token.total_licn_staked += licn_amount;
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        let lock_until = if tier.lock_duration_slots() > 0 {
            current_slot + tier.lock_duration_slots()
        } else {
            0
        };
        let lock_until_unix_seconds = if tier.lock_duration_seconds() > 0 {
            current_unix_seconds.saturating_add(tier.lock_duration_seconds())
        } else {
            0
        };

        if let Some(position) = self.positions.get_mut(&user) {
            if tier != position.lock_tier {
                return Err(format!(
                    "Cannot change lock tier on existing position (current: {:?}, requested: {:?}). Withdraw first.",
                    position.lock_tier, tier
                ));
            }
            position.st_licn_amount += st_licn_to_mint;
            position.licn_deposited += licn_amount;
            if lock_until > position.lock_until {
                position.lock_until = lock_until;
            }
            if lock_until_unix_seconds > position.lock_until_unix_seconds {
                position.lock_until_unix_seconds = lock_until_unix_seconds;
            }
        } else {
            self.positions.insert(
                user,
                StakingPosition {
                    owner: user,
                    st_licn_amount: st_licn_to_mint,
                    licn_deposited: licn_amount,
                    deposited_at: current_slot,
                    deposited_at_unix_seconds: current_unix_seconds,
                    rewards_earned: 0,
                    lock_tier: tier,
                    lock_until,
                    lock_until_unix_seconds,
                },
            );
        }

        Ok(st_licn_to_mint)
    }

    /// Request unstake (7-day cooldown)
    pub fn request_unstake(
        &mut self,
        user: Pubkey,
        st_licn_amount: u64,
        current_slot: u64,
    ) -> Result<UnstakeRequest, String> {
        // Check user has enough stLICN
        let position = self
            .positions
            .get_mut(&user)
            .ok_or_else(|| "No staking position found".to_string())?;

        // Enforce lock period — cannot unstake before lock expires
        if position.lock_until > 0 && current_slot < position.lock_until {
            let remaining_slots = position.lock_until - current_slot;
            let remaining_days = remaining_slots / 216_000;
            return Err(format!(
                "Position locked for {} more days ({} tier). Unlock at slot {}",
                remaining_days,
                position.lock_tier.display_name(),
                position.lock_until
            ));
        }

        if position.st_licn_amount < st_licn_amount {
            return Err(format!(
                "Insufficient stLICN: have {}, need {}",
                position.st_licn_amount, st_licn_amount
            ));
        }

        let (principal_out, rewards_out, licn_to_receive) =
            Self::split_position_value_for_unstake(position, st_licn_amount)?;

        // Burn stLICN from user
        position.st_licn_amount -= st_licn_amount;
        position.licn_deposited = position.licn_deposited.saturating_sub(principal_out);
        position.rewards_earned = position.rewards_earned.saturating_sub(rewards_out);

        // Update pool (stLICN burned, but LICN still locked for 7 days)
        self.st_licn_token.total_supply -= st_licn_amount;
        // M10 fix: decrement total_licn_staked at request time to prevent
        // exchange rate inflation during cooldown period
        self.st_licn_token.total_licn_staked = self
            .st_licn_token
            .total_licn_staked
            .saturating_sub(licn_to_receive);
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        // Create unstake request
        // AUDIT-FIX CP-4: Use constant from consensus module instead of hardcoded magic number
        let cooldown_slots = UNSTAKE_COOLDOWN_SLOTS;
        let request = UnstakeRequest {
            owner: user,
            st_licn_amount,
            licn_to_receive,
            requested_at: current_slot,
            claimable_at: current_slot + cooldown_slots,
            requested_at_unix_seconds: 0,
            claimable_at_unix_seconds: 0,
        };

        // Add to pending unstake requests
        self.unstake_requests
            .entry(user)
            .or_default()
            .push(request.clone());

        if let Some(position) = self.positions.get(&user) {
            if position.st_licn_amount == 0
                && position.licn_deposited == 0
                && position.rewards_earned == 0
            {
                self.positions.remove(&user);
            }
        }

        Ok(request)
    }

    pub fn request_unstake_at_time(
        &mut self,
        user: Pubkey,
        st_licn_amount: u64,
        current_slot: u64,
        current_unix_seconds: u64,
    ) -> Result<UnstakeRequest, String> {
        let position = self
            .positions
            .get_mut(&user)
            .ok_or_else(|| "No staking position found".to_string())?;

        if position.lock_until_unix_seconds > 0
            && current_unix_seconds < position.lock_until_unix_seconds
        {
            let remaining_seconds = position.lock_until_unix_seconds - current_unix_seconds;
            let remaining_days = remaining_seconds / SECONDS_PER_DAY;
            return Err(format!(
                "Position locked for {} more days ({} tier). Unlock at timestamp {}",
                remaining_days,
                position.lock_tier.display_name(),
                position.lock_until_unix_seconds
            ));
        } else if position.lock_until > 0 && current_slot < position.lock_until {
            let remaining_slots = position.lock_until - current_slot;
            let remaining_days = Self::legacy_slot_timestamp(remaining_slots) / SECONDS_PER_DAY;
            return Err(format!(
                "Position locked for {} more days ({} tier). Unlock at slot {}",
                remaining_days,
                position.lock_tier.display_name(),
                position.lock_until
            ));
        }

        if position.st_licn_amount < st_licn_amount {
            return Err(format!(
                "Insufficient stLICN: have {}, need {}",
                position.st_licn_amount, st_licn_amount
            ));
        }

        let (principal_out, rewards_out, licn_to_receive) =
            Self::split_position_value_for_unstake(position, st_licn_amount)?;

        position.st_licn_amount -= st_licn_amount;
        position.licn_deposited = position.licn_deposited.saturating_sub(principal_out);
        position.rewards_earned = position.rewards_earned.saturating_sub(rewards_out);

        self.st_licn_token.total_supply -= st_licn_amount;
        self.st_licn_token.total_licn_staked = self
            .st_licn_token
            .total_licn_staked
            .saturating_sub(licn_to_receive);
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        let cooldown_slots = UNSTAKE_COOLDOWN_SLOTS;
        let request = UnstakeRequest {
            owner: user,
            st_licn_amount,
            licn_to_receive,
            requested_at: current_slot,
            claimable_at: current_slot + cooldown_slots,
            requested_at_unix_seconds: current_unix_seconds,
            claimable_at_unix_seconds: current_unix_seconds
                .saturating_add(MOSSSTAKE_UNSTAKE_COOLDOWN_SECONDS),
        };

        self.unstake_requests
            .entry(user)
            .or_default()
            .push(request.clone());

        if let Some(position) = self.positions.get(&user) {
            if position.st_licn_amount == 0
                && position.licn_deposited == 0
                && position.rewards_earned == 0
            {
                self.positions.remove(&user);
            }
        }

        Ok(request)
    }

    /// Claim unstaked LICN (after cooldown)
    pub fn claim_unstake(&mut self, user: Pubkey, current_slot: u64) -> Result<u64, String> {
        let requests = self
            .unstake_requests
            .get_mut(&user)
            .ok_or_else(|| "No unstake requests found".to_string())?;

        // Find claimable requests
        let mut total_claimable = 0u64;
        let mut remaining_requests = Vec::new();

        for request in requests.drain(..) {
            if request.claimable_at <= current_slot {
                // Claimable!
                total_claimable += request.licn_to_receive;
            } else {
                // Still cooling down
                remaining_requests.push(request);
            }
        }

        if total_claimable == 0 {
            requests.extend(remaining_requests);
            return Err("No claimable unstake requests".to_string());
        }

        // Update pending requests
        if remaining_requests.is_empty() {
            self.unstake_requests.remove(&user);
        } else {
            self.unstake_requests.insert(user, remaining_requests);
        }

        // Update pool (LICN now released — total_licn_staked already decremented at request time)
        // M10 fix: removed redundant decrement that was here before
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        Ok(total_claimable)
    }

    pub fn claim_unstake_at_time(
        &mut self,
        user: Pubkey,
        current_slot: u64,
        current_unix_seconds: u64,
    ) -> Result<u64, String> {
        let requests = self
            .unstake_requests
            .get_mut(&user)
            .ok_or_else(|| "No unstake requests found".to_string())?;

        let mut total_claimable = 0u64;
        let mut remaining_requests = Vec::new();

        for request in requests.drain(..) {
            let claimable = if request.claimable_at_unix_seconds > 0 {
                request.claimable_at_unix_seconds <= current_unix_seconds
            } else {
                request.claimable_at <= current_slot
            };
            if claimable {
                total_claimable += request.licn_to_receive;
            } else {
                remaining_requests.push(request);
            }
        }

        if total_claimable == 0 {
            requests.extend(remaining_requests);
            return Err("No claimable unstake requests".to_string());
        }

        if remaining_requests.is_empty() {
            self.unstake_requests.remove(&user);
        } else {
            self.unstake_requests.insert(user, remaining_requests);
        }

        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        Ok(total_claimable)
    }

    /// Transfer stLICN between users
    pub fn transfer(
        &mut self,
        from: Pubkey,
        to: Pubkey,
        st_licn_amount: u64,
        current_slot: u64,
    ) -> Result<(), String> {
        if st_licn_amount == 0 {
            return Err("Cannot transfer 0 stLICN".to_string());
        }
        if from == to {
            return Err("Cannot transfer stLICN to self".to_string());
        }

        let sender_view = self
            .positions
            .get(&from)
            .ok_or_else(|| "Sender has no staking position".to_string())?;
        Self::assert_transferable_position(sender_view, current_slot)?;
        if sender_view.st_licn_amount < st_licn_amount {
            return Err(format!(
                "Insufficient stLICN: have {}, need {}",
                sender_view.st_licn_amount, st_licn_amount
            ));
        }
        if let Some(receiver) = self.positions.get(&to) {
            Self::assert_transferable_position(receiver, current_slot)?;
        }

        let (deposited_transfer, rewards_transfer, _) =
            Self::split_position_value_for_unstake(sender_view, st_licn_amount)?;

        // Deduct from sender after all failure checks have passed.
        let sender = self
            .positions
            .get_mut(&from)
            .ok_or_else(|| "Sender has no staking position".to_string())?;
        sender.st_licn_amount -= st_licn_amount;
        sender.licn_deposited = sender.licn_deposited.saturating_sub(deposited_transfer);
        sender.rewards_earned = sender.rewards_earned.saturating_sub(rewards_transfer);

        // Remove sender position if empty
        if sender.st_licn_amount == 0 && sender.licn_deposited == 0 && sender.rewards_earned == 0 {
            self.positions.remove(&from);
        }

        // Credit to receiver
        if let Some(receiver) = self.positions.get_mut(&to) {
            receiver.st_licn_amount += st_licn_amount;
            receiver.licn_deposited += deposited_transfer;
            receiver.rewards_earned += rewards_transfer;
        } else {
            self.positions.insert(
                to,
                StakingPosition {
                    owner: to,
                    st_licn_amount,
                    licn_deposited: deposited_transfer,
                    deposited_at: current_slot,
                    deposited_at_unix_seconds: 0,
                    rewards_earned: rewards_transfer,
                    lock_tier: LockTier::Flexible,
                    lock_until: 0,
                    lock_until_unix_seconds: 0,
                },
            );
        }

        Ok(())
    }

    pub fn transfer_at_time(
        &mut self,
        from: Pubkey,
        to: Pubkey,
        st_licn_amount: u64,
        current_slot: u64,
        current_unix_seconds: u64,
    ) -> Result<(), String> {
        if st_licn_amount == 0 {
            return Err("Cannot transfer 0 stLICN".to_string());
        }
        if from == to {
            return Err("Cannot transfer stLICN to self".to_string());
        }

        let sender_view = self
            .positions
            .get(&from)
            .ok_or_else(|| "Sender has no staking position".to_string())?;
        Self::assert_transferable_position_at_time(
            sender_view,
            current_slot,
            current_unix_seconds,
        )?;
        if sender_view.st_licn_amount < st_licn_amount {
            return Err(format!(
                "Insufficient stLICN: have {}, need {}",
                sender_view.st_licn_amount, st_licn_amount
            ));
        }
        if let Some(receiver) = self.positions.get(&to) {
            Self::assert_transferable_position_at_time(
                receiver,
                current_slot,
                current_unix_seconds,
            )?;
        }

        let (deposited_transfer, rewards_transfer, _) =
            Self::split_position_value_for_unstake(sender_view, st_licn_amount)?;

        let sender = self
            .positions
            .get_mut(&from)
            .ok_or_else(|| "Sender has no staking position".to_string())?;
        sender.st_licn_amount -= st_licn_amount;
        sender.licn_deposited = sender.licn_deposited.saturating_sub(deposited_transfer);
        sender.rewards_earned = sender.rewards_earned.saturating_sub(rewards_transfer);

        if sender.st_licn_amount == 0 && sender.licn_deposited == 0 && sender.rewards_earned == 0 {
            self.positions.remove(&from);
        }

        if let Some(receiver) = self.positions.get_mut(&to) {
            receiver.st_licn_amount += st_licn_amount;
            receiver.licn_deposited += deposited_transfer;
            receiver.rewards_earned += rewards_transfer;
        } else {
            self.positions.insert(
                to,
                StakingPosition {
                    owner: to,
                    st_licn_amount,
                    licn_deposited: deposited_transfer,
                    deposited_at: current_slot,
                    deposited_at_unix_seconds: current_unix_seconds,
                    rewards_earned: rewards_transfer,
                    lock_tier: LockTier::Flexible,
                    lock_until: 0,
                    lock_until_unix_seconds: 0,
                },
            );
        }

        Ok(())
    }

    /// Distribute rewards to all stakers (auto-compound)
    /// Uses tier-weighted distribution: locked stakers get boosted rewards.
    pub fn distribute_rewards(&mut self, total_rewards: u64) {
        if self.st_licn_token.total_supply == 0 {
            return;
        }

        // Add rewards to pool (increases exchange rate)
        self.st_licn_token.total_licn_staked += total_rewards;
        self.st_licn_token.exchange_rate_fp = self.st_licn_token.calculate_exchange_rate_fp();

        // Calculate total weighted stLICN across all positions
        let total_weighted = self.total_weighted_st_licn();

        if total_weighted == 0 {
            return;
        }

        // Distribute rewards proportionally to weighted stake
        // AUDIT-FIX CP-5: Track distributed sum and assign remainder dust to last position
        let mut distributed: u64 = 0;
        let position_count = self.positions.len();
        let mut idx = 0;
        for position in self.positions.values_mut() {
            idx += 1;
            let weighted = (position.st_licn_amount as u128
                * position.lock_tier.reward_multiplier_bp() as u128)
                / 10_000;
            let reward_share = if idx == position_count {
                // Last position gets remainder to avoid dust loss
                total_rewards.saturating_sub(distributed)
            } else {
                ((weighted * total_rewards as u128) / total_weighted) as u64
            };
            distributed += reward_share;
            position.rewards_earned += reward_share;
        }
    }

    /// Get user's position with current value
    pub fn get_position(&self, user: &Pubkey) -> Option<(StakingPosition, u64)> {
        self.positions.get(user).map(|pos| {
            let current_value = self.position_value(pos);
            (pos.clone(), current_value)
        })
    }

    /// Get pending unstake requests for user
    pub fn get_unstake_requests(&self, user: &Pubkey) -> Vec<UnstakeRequest> {
        self.unstake_requests.get(user).cloned().unwrap_or_default()
    }

    pub fn canonical_snapshot_bytes(&self) -> Result<Vec<u8>, String> {
        let positions = self
            .positions
            .iter()
            .map(|(owner, position)| (*owner, position.clone()))
            .collect();

        let mut unstake_requests: Vec<_> = self
            .unstake_requests
            .iter()
            .map(|(owner, requests)| (*owner, requests.clone()))
            .collect();
        unstake_requests.sort_by_key(|(owner, _)| owner.0);

        let snapshot = MossStakePoolSnapshotV1 {
            version: 1,
            st_licn_token: self.st_licn_token.clone(),
            positions,
            unstake_requests,
            total_validators: self.total_validators,
            average_apy_bp: self.average_apy_bp,
        };

        serialize_legacy_bincode(&snapshot, "MossStake canonical snapshot")
    }

    pub fn from_canonical_snapshot_bytes(data: &[u8]) -> Result<Self, String> {
        let snapshot: MossStakePoolSnapshotV1 = deserialize_legacy_bincode_strict(
            data,
            data.len() as u64,
            "MossStake canonical snapshot v1",
        )?;
        if snapshot.version != 1 {
            return Err(format!(
                "unsupported MossStake snapshot version {}",
                snapshot.version
            ));
        }

        Ok(Self {
            st_licn_token: snapshot.st_licn_token,
            positions: snapshot.positions.into_iter().collect(),
            unstake_requests: snapshot.unstake_requests.into_iter().collect(),
            total_validators: snapshot.total_validators,
            average_apy_bp: snapshot.average_apy_bp,
        })
    }

    /// Calculate current APY in basis points (10000 = 100.00%)
    pub fn calculate_apy_bp(&self, blocks_per_day: u64, block_reward: u64) -> u64 {
        if self.st_licn_token.total_licn_staked == 0 {
            return 0;
        }
        let daily_rewards = blocks_per_day as u128 * block_reward as u128;
        let annual_rewards = daily_rewards * 365;
        // APY in basis points: (annual / staked) * 10000
        ((annual_rewards * 10_000) / self.st_licn_token.total_licn_staked as u128) as u64
    }

    /// Estimate tier APY against the current weighted pool composition.
    ///
    /// Multipliers affect relative reward share, so they do not simply multiply
    /// the pool average when all positions use the same tier. This mirrors
    /// staking systems that divide a fixed reward budget by weighted stake.
    pub fn calculate_tier_apy_bp(
        &self,
        blocks_per_day: u64,
        block_reward: u64,
        tier: LockTier,
    ) -> u64 {
        let total_weighted = self.total_weighted_st_licn();
        if total_weighted == 0 || self.st_licn_token.exchange_rate_fp == 0 {
            return 0;
        }

        let daily_rewards = blocks_per_day as u128 * block_reward as u128;
        let annual_rewards = daily_rewards * 365;
        ((annual_rewards * tier.reward_multiplier_bp() as u128 * RATE_PRECISION)
            / (total_weighted * self.st_licn_token.exchange_rate_fp as u128)) as u64
    }

    /// Calculate APY as f64 percentage (for display/API only — NOT for consensus)
    pub fn calculate_apy_display(&self, blocks_per_day: u64, block_reward: u64) -> f64 {
        self.calculate_apy_bp(blocks_per_day, block_reward) as f64 / 100.0
    }

    /// Compute a deterministic hash of the entire MossStake pool.
    ///
    /// `positions` is already a BTreeMap (sorted).  `unstake_requests` is a
    /// HashMap, so we collect into a sorted BTreeMap before serializing.
    pub fn canonical_hash(&self) -> crate::Hash {
        #[derive(Serialize)]
        struct SlotOnlyPosition {
            owner: Pubkey,
            st_licn_amount: u64,
            licn_deposited: u64,
            deposited_at: u64,
            rewards_earned: u64,
            lock_tier: LockTier,
            lock_until: u64,
        }

        #[derive(Serialize)]
        struct SlotOnlyUnstakeRequest {
            owner: Pubkey,
            st_licn_amount: u64,
            licn_to_receive: u64,
            requested_at: u64,
            claimable_at: u64,
        }

        let positions: BTreeMap<&Pubkey, SlotOnlyPosition> = self
            .positions
            .iter()
            .map(|(key, position)| {
                (
                    key,
                    SlotOnlyPosition {
                        owner: position.owner,
                        st_licn_amount: position.st_licn_amount,
                        licn_deposited: position.licn_deposited,
                        deposited_at: position.deposited_at,
                        rewards_earned: position.rewards_earned,
                        lock_tier: position.lock_tier,
                        lock_until: position.lock_until,
                    },
                )
            })
            .collect();

        let sorted_unstake: BTreeMap<&Pubkey, Vec<SlotOnlyUnstakeRequest>> = self
            .unstake_requests
            .iter()
            .map(|(key, requests)| {
                (
                    key,
                    requests
                        .iter()
                        .map(|request| SlotOnlyUnstakeRequest {
                            owner: request.owner,
                            st_licn_amount: request.st_licn_amount,
                            licn_to_receive: request.licn_to_receive,
                            requested_at: request.requested_at,
                            claimable_at: request.claimable_at,
                        })
                        .collect(),
                )
            })
            .collect();

        let data = serialize_legacy_bincode(
            &(
                0x04u8, // domain separator
                &self.st_licn_token,
                &positions,
                &sorted_unstake,
                self.total_validators,
                self.average_apy_bp,
            ),
            "MossStake canonical hash",
        )
        .unwrap_or_default();

        crate::Hash::hash(&data)
    }

    /// Legacy v0.5.93 hash including wall-clock fields. Kept only so an
    /// un-migrated database can still compute its historical root before the
    /// slot-only migration marker is set.
    pub fn legacy_canonical_hash(&self) -> crate::Hash {
        let sorted_unstake: BTreeMap<&Pubkey, &Vec<UnstakeRequest>> =
            self.unstake_requests.iter().collect();

        let data = serialize_legacy_bincode(
            &(
                0x04u8,
                &self.st_licn_token,
                &self.positions,
                &sorted_unstake,
                self.total_validators,
                self.average_apy_bp,
            ),
            "MossStake legacy canonical hash",
        )
        .unwrap_or_default();

        crate::Hash::hash(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_snapshot_bytes_ignore_unstake_hashmap_order() {
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();
        let bob = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        let mut first = MossStakePool::new();
        first.stake(alice, 1_000, 0).unwrap();
        first.stake(bob, 2_000, 0).unwrap();
        first.request_unstake(alice, 1_000, 0).unwrap();
        first.request_unstake(bob, 2_000, 0).unwrap();

        let mut second = MossStakePool::new();
        second.stake(bob, 2_000, 0).unwrap();
        second.stake(alice, 1_000, 0).unwrap();
        second.request_unstake(bob, 2_000, 0).unwrap();
        second.request_unstake(alice, 1_000, 0).unwrap();

        let first_bytes = first.canonical_snapshot_bytes().unwrap();
        let second_bytes = second.canonical_snapshot_bytes().unwrap();
        assert_eq!(first_bytes, second_bytes);

        let restored = MossStakePool::from_canonical_snapshot_bytes(&first_bytes).unwrap();
        assert_eq!(restored.canonical_hash(), first.canonical_hash());
    }

    #[test]
    fn test_liquid_staking_flow() {
        let mut pool = MossStakePool::new();
        let user = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        // Stake 1000 LICN
        let st_licn_amount_out = pool.stake(user, 1000, 0).unwrap();
        assert_eq!(st_licn_amount_out, 1000); // 1:1 initially

        // Simulate rewards
        pool.distribute_rewards(100); // 10% rewards

        // Exchange rate should increase (> 1.0x, i.e., > RATE_PRECISION)
        assert!(pool.st_licn_token.calculate_exchange_rate_fp() > RATE_PRECISION as u64);

        // User's position worth more now
        let (_position, current_value) = pool.get_position(&user).unwrap();
        assert_eq!(current_value, 1100); // Original 1000 + 100 rewards

        // Request unstake
        let request = pool.request_unstake(user, st_licn_amount_out, 0).unwrap();
        assert_eq!(request.licn_to_receive, 1100); // Gets rewards!

        // Try to claim immediately (should fail - cooldown)
        assert!(pool.claim_unstake(user, 100).is_err());

        // Try just before cooldown ends (should fail)
        assert!(pool.claim_unstake(user, 1_511_999).is_err());

        // Claim after cooldown (7 days = 1,512,000 slots)
        let claimed = pool.claim_unstake(user, 1_512_001).unwrap();
        assert_eq!(claimed, 1100);
    }

    #[test]
    fn test_stlicn_transfer() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();
        let bob = Pubkey::from_base58("BwVDmnwtfVBiRYB4iWxWrb5M9fAfQD9hbMmnQMw3MRvV").unwrap();

        // Alice stakes 1000 LICN
        let st_licn_amount_out = pool.stake(alice, 1000, 0).unwrap();
        assert_eq!(st_licn_amount_out, 1000);

        // Transfer 400 stLICN from Alice to Bob
        pool.transfer(alice, bob, 400, 100).unwrap();

        // Check balances
        let (alice_pos, _) = pool.get_position(&alice).unwrap();
        assert_eq!(alice_pos.st_licn_amount, 600);

        let (bob_pos, _) = pool.get_position(&bob).unwrap();
        assert_eq!(bob_pos.st_licn_amount, 400);

        // Transfer more than available should fail
        assert!(pool.transfer(alice, bob, 700, 100).is_err());

        // Transfer to self should fail
        assert!(pool.transfer(alice, alice, 100, 100).is_err());

        // Transfer 0 should fail
        assert!(pool.transfer(alice, bob, 0, 100).is_err());

        // Transfer all remaining from Alice to Bob
        pool.transfer(alice, bob, 600, 200).unwrap();
        assert!(pool.get_position(&alice).is_none()); // Alice removed
        let (bob_pos, _) = pool.get_position(&bob).unwrap();
        assert_eq!(bob_pos.st_licn_amount, 1000);
    }

    /// P9-CORE-01: Verify distribute_rewards is deterministic across multiple stakers.
    /// With BTreeMap, the "last position gets remainder" dust always goes to the
    /// lexicographically highest Pubkey, ensuring cross-validator consistency.
    #[test]
    fn test_distribute_rewards_deterministic() {
        let mut pool = MossStakePool::new();
        // Create 3 stakers with known pubkeys
        let pk_a = Pubkey::from_base58("11111111111111111111111111111112").unwrap();
        let pk_b = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();
        let pk_c = Pubkey::from_base58("BwVDmnwtfVBiRYB4iWxWrb5M9fAfQD9hbMmnQMw3MRvV").unwrap();

        pool.stake(pk_a, 100, 0).unwrap();
        pool.stake(pk_b, 100, 0).unwrap();
        pool.stake(pk_c, 100, 0).unwrap();

        // Distribute 10 spores that don't divide evenly by 3
        pool.distribute_rewards(10);

        let a_rewards = pool.get_position(&pk_a).unwrap().0.rewards_earned;
        let b_rewards = pool.get_position(&pk_b).unwrap().0.rewards_earned;
        let c_rewards = pool.get_position(&pk_c).unwrap().0.rewards_earned;

        // Total must be exactly 10 (no dust lost)
        assert_eq!(a_rewards + b_rewards + c_rewards, 10);

        // BTreeMap sorts by bytes, so iteration order is deterministic.
        // The last key (lexicographically highest) gets the remainder dust.
        // Run twice to confirm determinism:
        let mut pool2 = MossStakePool::new();
        pool2.stake(pk_c, 100, 0).unwrap(); // insert order swapped
        pool2.stake(pk_a, 100, 0).unwrap();
        pool2.stake(pk_b, 100, 0).unwrap();
        pool2.distribute_rewards(10);

        assert_eq!(
            pool2.get_position(&pk_a).unwrap().0.rewards_earned,
            a_rewards
        );
        assert_eq!(
            pool2.get_position(&pk_b).unwrap().0.rewards_earned,
            b_rewards
        );
        assert_eq!(
            pool2.get_position(&pk_c).unwrap().0.rewards_earned,
            c_rewards
        );

        // Verify positions field is BTreeMap (deterministic)
        assert!(pool
            .positions
            .keys()
            .collect::<Vec<_>>()
            .windows(2)
            .all(|w| w[0] <= w[1]));
    }

    #[test]
    fn test_weighted_rewards_are_redeemed_by_position() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();
        let bob = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        pool.stake_with_tier(alice, 1_000, 0, LockTier::Flexible)
            .unwrap();
        pool.stake_with_tier(bob, 1_000, 0, LockTier::Lock30)
            .unwrap();

        pool.distribute_rewards(260);

        let (_, alice_value) = pool.get_position(&alice).unwrap();
        let (_, bob_value) = pool.get_position(&bob).unwrap();
        assert_eq!(alice_value, 1_100);
        assert_eq!(bob_value, 1_160);

        let alice_request = pool.request_unstake(alice, 1_000, 0).unwrap();
        assert_eq!(alice_request.licn_to_receive, 1_100);

        let bob_request = pool
            .request_unstake(bob, 1_000, LockTier::Lock30.lock_duration_slots())
            .unwrap();
        assert_eq!(bob_request.licn_to_receive, 1_160);
        assert_eq!(pool.st_licn_token.total_licn_staked, 0);
    }

    #[test]
    fn test_mossstake_timing_is_slot_only() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();

        pool.stake_with_tier(alice, 1_000, 10, LockTier::Lock30)
            .unwrap();
        let unlock_slot = 10 + LockTier::Lock30.lock_duration_slots();

        assert!(pool.request_unstake(alice, 1_000, unlock_slot - 1).is_err());
        let request = pool.request_unstake(alice, 1_000, unlock_slot).unwrap();
        assert_eq!(request.licn_to_receive, 1_000);

        assert!(pool
            .claim_unstake(alice, unlock_slot + UNSTAKE_COOLDOWN_SLOTS - 1)
            .is_err());
        let claimed = pool
            .claim_unstake(alice, unlock_slot + UNSTAKE_COOLDOWN_SLOTS)
            .unwrap();
        assert_eq!(claimed, 1_000);
    }

    #[test]
    fn test_legacy_wall_clock_fields_do_not_affect_canonical_hash() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();

        pool.stake_with_tier(alice, 1_000, 42, LockTier::Lock30)
            .unwrap();
        pool.request_unstake(alice, 500, 42 + LockTier::Lock30.lock_duration_slots())
            .unwrap();

        let base_hash = pool.canonical_hash();
        let mut legacy_pool = pool.clone();
        {
            let position = legacy_pool.positions.get_mut(&alice).unwrap();
            position.deposited_at_unix_seconds = 1_700_000_000;
            position.lock_until_unix_seconds = 1_702_592_000;
        }
        {
            let request = legacy_pool
                .unstake_requests
                .get_mut(&alice)
                .unwrap()
                .first_mut()
                .unwrap();
            request.requested_at_unix_seconds = 1_702_592_000;
            request.claimable_at_unix_seconds = 1_703_196_800;
        }

        assert_eq!(base_hash, legacy_pool.canonical_hash());
        let changed = legacy_pool.clear_wall_clock_times();
        assert!(changed);
        assert_eq!(base_hash, legacy_pool.canonical_hash());
        assert!(legacy_pool
            .positions
            .values()
            .all(|position| position.deposited_at_unix_seconds == 0
                && position.lock_until_unix_seconds == 0));
        assert!(legacy_pool.unstake_requests.values().all(|requests| {
            requests.iter().all(|request| {
                request.requested_at_unix_seconds == 0 && request.claimable_at_unix_seconds == 0
            })
        }));
    }

    #[test]
    fn test_flexible_transfer_carries_reward_backing_pro_rata() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();
        let bob = Pubkey::from_base58("BwVDmnwtfVBiRYB4iWxWrb5M9fAfQD9hbMmnQMw3MRvV").unwrap();

        pool.stake(alice, 1_000, 0).unwrap();
        pool.distribute_rewards(100);
        pool.transfer(alice, bob, 400, 10).unwrap();

        let (alice_pos, alice_value) = pool.get_position(&alice).unwrap();
        let (bob_pos, bob_value) = pool.get_position(&bob).unwrap();
        assert_eq!(alice_pos.licn_deposited, 600);
        assert_eq!(alice_pos.rewards_earned, 60);
        assert_eq!(alice_value, 660);
        assert_eq!(bob_pos.licn_deposited, 400);
        assert_eq!(bob_pos.rewards_earned, 40);
        assert_eq!(bob_value, 440);

        let request = pool.request_unstake(bob, 400, 20).unwrap();
        assert_eq!(request.licn_to_receive, 440);
    }

    #[test]
    fn test_locked_tier_positions_are_not_transferable() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();
        let bob = Pubkey::from_base58("BwVDmnwtfVBiRYB4iWxWrb5M9fAfQD9hbMmnQMw3MRvV").unwrap();

        pool.stake_with_tier(alice, 1_000, 0, LockTier::Lock30)
            .unwrap();
        let err = pool.transfer(alice, bob, 100, 10).unwrap_err();
        assert!(
            err.contains("not transferable"),
            "locked-tier transfer should be rejected, got {err}"
        );
    }

    #[test]
    fn test_transfer_to_locked_position_does_not_mutate_sender() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();
        let bob = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        pool.stake(alice, 1_000, 0).unwrap();
        pool.distribute_rewards(100);
        pool.stake_with_tier(bob, 1_000, 0, LockTier::Lock30)
            .unwrap();

        let before = pool.get_position(&alice).unwrap().0;
        let err = pool.transfer(alice, bob, 400, 10).unwrap_err();
        assert!(
            err.contains("not transferable"),
            "transfer into locked position should be rejected, got {err}"
        );
        let after = pool.get_position(&alice).unwrap().0;
        assert_eq!(after.st_licn_amount, before.st_licn_amount);
        assert_eq!(after.licn_deposited, before.licn_deposited);
        assert_eq!(after.rewards_earned, before.rewards_earned);
    }

    #[test]
    fn test_tier_apy_uses_weighted_pool_composition() {
        let mut pool = MossStakePool::new();
        let alice = Pubkey::from_base58("11111111111111111111111111111112").unwrap();
        let bob = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        pool.stake_with_tier(alice, 1_000, 0, LockTier::Lock30)
            .unwrap();
        pool.stake_with_tier(bob, 1_000, 0, LockTier::Lock30)
            .unwrap();

        let average_apy = pool.calculate_apy_bp(1, 100);
        let lock30_apy = pool.calculate_tier_apy_bp(1, 100, LockTier::Lock30);
        let flexible_apy = pool.calculate_tier_apy_bp(1, 100, LockTier::Flexible);

        assert_eq!(
            lock30_apy, average_apy,
            "when every position has the same multiplier, the multiplier cancels out"
        );
        assert!(
            flexible_apy < lock30_apy,
            "a lower multiplier should estimate a lower APY in a locked pool"
        );
    }
}
