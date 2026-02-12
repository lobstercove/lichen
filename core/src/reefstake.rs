// MoltChain ReefStake - Liquid Staking Protocol
// Stake MOLT, receive stMOLT (liquid receipt token)

use crate::Pubkey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// stMOLT token - liquid staking receipt
/// T3.2/T6.2 fix: All math is integer-only (fixed-point with PRECISION denominator).
/// No floating-point is used anywhere in consensus-critical code.
///
/// Exchange rate is stored as basis points: rate_bp = (total_molt * RATE_PRECISION) / total_supply
/// RATE_PRECISION = 1_000_000_000 (1e9) to match shell precision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StMoltToken {
    pub total_supply: u64,      // Total stMOLT in circulation
    pub total_molt_staked: u64, // Total MOLT backing stMOLT
    /// Exchange rate in fixed-point: (MOLT_per_stMOLT * RATE_PRECISION)
    /// e.g., 1_000_000_000 = 1.0x, 1_100_000_000 = 1.1x
    pub exchange_rate_fp: u64,
}

/// Fixed-point precision for exchange rate (1e9)
const RATE_PRECISION: u128 = 1_000_000_000;

impl Default for StMoltToken {
    fn default() -> Self {
        Self::new()
    }
}

impl StMoltToken {
    pub fn new() -> Self {
        Self {
            total_supply: 0,
            total_molt_staked: 0,
            exchange_rate_fp: RATE_PRECISION as u64, // 1.0 initially
        }
    }

    /// Calculate exchange rate as fixed-point (MOLT per stMOLT * RATE_PRECISION)
    /// Increases as rewards accumulate.
    pub fn calculate_exchange_rate_fp(&self) -> u64 {
        if self.total_supply == 0 {
            RATE_PRECISION as u64
        } else {
            // Use u128 to avoid overflow: (total_molt * PRECISION) / total_supply
            ((self.total_molt_staked as u128 * RATE_PRECISION) / self.total_supply as u128) as u64
        }
    }

    /// Calculate exchange rate as f64 (for display/API only — NOT for consensus math)
    pub fn exchange_rate_display(&self) -> f64 {
        self.exchange_rate_fp as f64 / RATE_PRECISION as f64
    }

    /// Calculate stMOLT to mint for given MOLT amount (integer math only)
    pub fn molt_to_st_molt(&self, molt_amount: u64) -> u64 {
        if self.total_supply == 0 {
            molt_amount
        } else {
            // st_molt = (molt * PRECISION) / exchange_rate_fp
            let rate = self.exchange_rate_fp.max(1) as u128;
            ((molt_amount as u128 * RATE_PRECISION) / rate) as u64
        }
    }

    /// Calculate MOLT to return for given stMOLT amount (integer math only)
    pub fn st_molt_to_molt(&self, st_molt_amount: u64) -> u64 {
        // molt = (st_molt * exchange_rate_fp) / PRECISION
        ((st_molt_amount as u128 * self.exchange_rate_fp as u128) / RATE_PRECISION) as u64
    }
}

/// User's staking position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingPosition {
    pub owner: Pubkey,
    pub st_molt_amount: u64, // stMOLT balance
    pub molt_deposited: u64, // Original MOLT deposited
    pub deposited_at: u64,   // Slot when deposited
    pub rewards_earned: u64, // Accumulated rewards (auto-compound)
}

/// Unstaking request (7-day cooldown)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnstakeRequest {
    pub owner: Pubkey,
    pub st_molt_amount: u64,  // stMOLT being unstaked
    pub molt_to_receive: u64, // MOLT to receive (locked rate)
    pub requested_at: u64,    // Slot when requested
    pub claimable_at: u64,    // Slot when can claim (requested + 7 days)
}

/// ReefStake liquid staking pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReefStakePool {
    pub st_molt_token: StMoltToken,
    #[serde(with = "pubkey_map_serde")]
    pub positions: HashMap<Pubkey, StakingPosition>,
    #[serde(with = "pubkey_map_serde")]
    pub unstake_requests: HashMap<Pubkey, Vec<UnstakeRequest>>,
    pub total_validators: u64, // Number of validators staked to
    /// Average APY in basis points (10000 = 100.00%)
    pub average_apy_bp: u64,
}

impl Default for ReefStakePool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReefStakePool {
    pub fn new() -> Self {
        Self {
            st_molt_token: StMoltToken::new(),
            positions: HashMap::new(),
            unstake_requests: HashMap::new(),
            total_validators: 0,
            average_apy_bp: 0,
        }
    }

    /// Stake MOLT, mint stMOLT
    pub fn stake(
        &mut self,
        user: Pubkey,
        molt_amount: u64,
        current_slot: u64,
    ) -> Result<u64, String> {
        if molt_amount == 0 {
            return Err("Cannot stake 0 MOLT".to_string());
        }

        // Calculate stMOLT to mint
        let st_molt_to_mint = self.st_molt_token.molt_to_st_molt(molt_amount);

        // Update pool
        self.st_molt_token.total_supply += st_molt_to_mint;
        self.st_molt_token.total_molt_staked += molt_amount;
        self.st_molt_token.exchange_rate_fp = self.st_molt_token.calculate_exchange_rate_fp();

        // Update user position
        if let Some(position) = self.positions.get_mut(&user) {
            position.st_molt_amount += st_molt_to_mint;
            position.molt_deposited += molt_amount;
        } else {
            self.positions.insert(
                user,
                StakingPosition {
                    owner: user,
                    st_molt_amount: st_molt_to_mint,
                    molt_deposited: molt_amount,
                    deposited_at: current_slot,
                    rewards_earned: 0,
                },
            );
        }

        Ok(st_molt_to_mint)
    }

    /// Request unstake (7-day cooldown)
    pub fn request_unstake(
        &mut self,
        user: Pubkey,
        st_molt_amount: u64,
        current_slot: u64,
    ) -> Result<UnstakeRequest, String> {
        // Check user has enough stMOLT
        let position = self
            .positions
            .get_mut(&user)
            .ok_or_else(|| "No staking position found".to_string())?;

        if position.st_molt_amount < st_molt_amount {
            return Err(format!(
                "Insufficient stMOLT: have {}, need {}",
                position.st_molt_amount, st_molt_amount
            ));
        }

        // Calculate MOLT to receive (lock exchange rate now)
        let molt_to_receive = self.st_molt_token.st_molt_to_molt(st_molt_amount);

        // Burn stMOLT from user
        position.st_molt_amount -= st_molt_amount;

        // Update pool (stMOLT burned, but MOLT still locked for 7 days)
        self.st_molt_token.total_supply -= st_molt_amount;
        // M10 fix: decrement total_molt_staked at request time to prevent
        // exchange rate inflation during cooldown period
        self.st_molt_token.total_molt_staked = self
            .st_molt_token
            .total_molt_staked
            .saturating_sub(molt_to_receive);
        self.st_molt_token.exchange_rate_fp = self.st_molt_token.calculate_exchange_rate_fp();

        // Create unstake request (7 days at 400ms/slot = 86400*7/0.4 = 1,512,000 slots)
        let cooldown_slots = 1_512_000; // 7 days
        let request = UnstakeRequest {
            owner: user,
            st_molt_amount,
            molt_to_receive,
            requested_at: current_slot,
            claimable_at: current_slot + cooldown_slots,
        };

        // Add to pending unstake requests
        self.unstake_requests
            .entry(user)
            .or_default()
            .push(request.clone());

        Ok(request)
    }

    /// Claim unstaked MOLT (after cooldown)
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
                total_claimable += request.molt_to_receive;
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

        // Update pool (MOLT now released — total_molt_staked already decremented at request time)
        // M10 fix: removed redundant decrement that was here before
        self.st_molt_token.exchange_rate_fp = self.st_molt_token.calculate_exchange_rate_fp();

        Ok(total_claimable)
    }

    /// Transfer stMOLT between users
    pub fn transfer(
        &mut self,
        from: Pubkey,
        to: Pubkey,
        st_molt_amount: u64,
        current_slot: u64,
    ) -> Result<(), String> {
        if st_molt_amount == 0 {
            return Err("Cannot transfer 0 stMOLT".to_string());
        }
        if from == to {
            return Err("Cannot transfer stMOLT to self".to_string());
        }

        // Deduct from sender
        let sender = self
            .positions
            .get_mut(&from)
            .ok_or_else(|| "Sender has no staking position".to_string())?;
        if sender.st_molt_amount < st_molt_amount {
            return Err(format!(
                "Insufficient stMOLT: have {}, need {}",
                sender.st_molt_amount, st_molt_amount
            ));
        }
        sender.st_molt_amount -= st_molt_amount;
        // Proportionally reduce the deposited tracking
        let proportion = if sender.st_molt_amount == 0 {
            // Sent everything: transfer the remaining molt_deposited proportion
            let deposited_transfer = sender.molt_deposited;
            sender.molt_deposited = 0;
            deposited_transfer
        } else {
            // Partial: pro-rata
            let total_before = sender.st_molt_amount + st_molt_amount;
            if total_before == 0 {
                0
            } else {
                let transfer_deposited =
                    ((st_molt_amount as u128 * sender.molt_deposited as u128) / total_before as u128)
                        as u64;
                sender.molt_deposited -= transfer_deposited;
                transfer_deposited
            }
        };

        // Remove sender position if empty
        if sender.st_molt_amount == 0 && sender.molt_deposited == 0 {
            self.positions.remove(&from);
        }

        // Credit to receiver
        if let Some(receiver) = self.positions.get_mut(&to) {
            receiver.st_molt_amount += st_molt_amount;
            receiver.molt_deposited += proportion;
        } else {
            self.positions.insert(
                to,
                StakingPosition {
                    owner: to,
                    st_molt_amount,
                    molt_deposited: proportion,
                    deposited_at: current_slot,
                    rewards_earned: 0,
                },
            );
        }

        Ok(())
    }

    /// Distribute rewards to all stakers (auto-compound)
    pub fn distribute_rewards(&mut self, total_rewards: u64) {
        if self.st_molt_token.total_supply == 0 {
            return;
        }

        // Add rewards to pool (increases exchange rate)
        self.st_molt_token.total_molt_staked += total_rewards;
        self.st_molt_token.exchange_rate_fp = self.st_molt_token.calculate_exchange_rate_fp();

        // Update each user's rewards_earned for tracking (integer proportional math)
        // L3 note: integer division dust is lost in the tracking field only.
        // The actual MOLT value is preserved in total_molt_staked / exchange rate.
        for position in self.positions.values_mut() {
            // share = (position.st_molt * total_rewards) / total_supply  (integer division)
            let reward_share = ((position.st_molt_amount as u128 * total_rewards as u128)
                / self.st_molt_token.total_supply as u128) as u64;
            position.rewards_earned += reward_share;
        }
    }

    /// Get user's position with current value
    pub fn get_position(&self, user: &Pubkey) -> Option<(StakingPosition, u64)> {
        self.positions.get(user).map(|pos| {
            let current_value = self.st_molt_token.st_molt_to_molt(pos.st_molt_amount);
            (pos.clone(), current_value)
        })
    }

    /// Get pending unstake requests for user
    pub fn get_unstake_requests(&self, user: &Pubkey) -> Vec<UnstakeRequest> {
        self.unstake_requests.get(user).cloned().unwrap_or_default()
    }

    /// Calculate current APY in basis points (10000 = 100.00%)
    pub fn calculate_apy_bp(&self, blocks_per_day: u64, block_reward: u64) -> u64 {
        if self.st_molt_token.total_molt_staked == 0 {
            return 0;
        }
        let daily_rewards = blocks_per_day as u128 * block_reward as u128;
        let annual_rewards = daily_rewards * 365;
        // APY in basis points: (annual / staked) * 10000
        ((annual_rewards * 10_000) / self.st_molt_token.total_molt_staked as u128) as u64
    }

    /// Calculate APY as f64 percentage (for display/API only — NOT for consensus)
    pub fn calculate_apy_display(&self, blocks_per_day: u64, block_reward: u64) -> f64 {
        self.calculate_apy_bp(blocks_per_day, block_reward) as f64 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_liquid_staking_flow() {
        let mut pool = ReefStakePool::new();
        let user = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();

        // Stake 1000 MOLT
        let st_molt = pool.stake(user, 1000, 0).unwrap();
        assert_eq!(st_molt, 1000); // 1:1 initially

        // Simulate rewards
        pool.distribute_rewards(100); // 10% rewards

        // Exchange rate should increase (> 1.0x, i.e., > RATE_PRECISION)
        assert!(pool.st_molt_token.calculate_exchange_rate_fp() > RATE_PRECISION as u64);

        // User's position worth more now
        let (_position, current_value) = pool.get_position(&user).unwrap();
        assert_eq!(current_value, 1100); // Original 1000 + 100 rewards

        // Request unstake
        let request = pool.request_unstake(user, st_molt, 0).unwrap();
        assert_eq!(request.molt_to_receive, 1100); // Gets rewards!

        // Try to claim immediately (should fail - cooldown)
        assert!(pool.claim_unstake(user, 100).is_err());

        // Try just before cooldown ends (should fail)
        assert!(pool.claim_unstake(user, 1_511_999).is_err());

        // Claim after cooldown (7 days = 1,512,000 slots)
        let claimed = pool.claim_unstake(user, 1_512_001).unwrap();
        assert_eq!(claimed, 1100);
    }

    #[test]
    fn test_stmolt_transfer() {
        let mut pool = ReefStakePool::new();
        let alice = Pubkey::from_base58("6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H").unwrap();
        let bob = Pubkey::from_base58("BwVDmnwtfVBiRYB4iWxWrb5M9fAfQD9hbMmnQMw3MRvV").unwrap();

        // Alice stakes 1000 MOLT
        let st_molt = pool.stake(alice, 1000, 0).unwrap();
        assert_eq!(st_molt, 1000);

        // Transfer 400 stMOLT from Alice to Bob
        pool.transfer(alice, bob, 400, 100).unwrap();

        // Check balances
        let (alice_pos, _) = pool.get_position(&alice).unwrap();
        assert_eq!(alice_pos.st_molt_amount, 600);

        let (bob_pos, _) = pool.get_position(&bob).unwrap();
        assert_eq!(bob_pos.st_molt_amount, 400);

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
        assert_eq!(bob_pos.st_molt_amount, 1000);
    }
}
