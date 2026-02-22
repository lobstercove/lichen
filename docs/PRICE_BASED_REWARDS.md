# Price-Based Validator Reward Adjustment

## Problem

Similar to transaction fees, validator rewards need to adjust based on $MOLT price to maintain consistent USD-equivalent rewards:

**Current rewards (fixed MOLT):**
```
Transaction block: 0.1 MOLT
Heartbeat block: 0.05 MOLT
```

**Problem scenarios:**
- $MOLT = $0.01 → Validator earns $0.0018/block (too low)
- $MOLT = $10 → Validator earns $1.80/block (too high?)
- $MOLT = $100 → Validator earns $18/block (excessive)

**Target**: Maintain 0.002 - $0.50 per transaction block regardless of $MOLT price

## Solution Design

### 1. Reward Adjustment Config

```rust
// core/src/consensus.rs

pub struct RewardConfig {
    /// Base reward in MOLT (at reference price)
    pub base_transaction_reward: u64,     // 100_000_000 shells (0.1 MOLT)
    pub base_heartbeat_reward: u64,       // 50_000_000 shells (0.05 MOLT)
    
    /// Price adjustment parameters
    pub reference_price_usd: f64,         // 1.00 USD (initial target)
    pub target_reward_usd: f64,           // 0.10 USD per TX block
    pub target_heartbeat_usd: f64,        // 0.05 USD per heartbeat
    
    /// Adjustment bounds
    pub max_adjustment_multiplier: f64,   // 10x max (prevent extreme swings)
    pub min_adjustment_multiplier: f64,   // 0.1x min
    
    /// Update frequency
    pub adjustment_frequency_slots: u64,  // Every 216000 slots (~24h)
    
    /// Current multiplier (updated dynamically)
    pub current_multiplier: f64,          // 1.0 initially
    
    /// Last update
    pub last_update_slot: u64,
}

impl RewardConfig {
    pub fn new() -> Self {
        RewardConfig {
            base_transaction_reward: 100_000_000,
            base_heartbeat_reward: 50_000_000,
            reference_price_usd: 1.0,
            target_reward_usd: 0.10,
            target_heartbeat_usd: 0.05,
            max_adjustment_multiplier: 10.0,
            min_adjustment_multiplier: 0.1,
            adjustment_frequency_slots: 216_000,
            current_multiplier: 1.0,
            last_update_slot: 0,
        }
    }
    
    /// Calculate adjusted reward based on current MOLT price
    pub fn calculate_adjusted_reward(
        &self,
        base_reward: u64,
        molt_price_usd: f64,
    ) -> u64 {
        // Formula: adjusted = base * (reference_price / current_price)
        // Example: $MOLT = $0.01, reference = $1.00
        // Multiplier = 1.00 / 0.01 = 100x
        // 0.1 MOLT * 100 = 10 MOLT ($0.10 USD worth)
        
        let multiplier = (self.reference_price_usd / molt_price_usd)
            .max(self.min_adjustment_multiplier)
            .min(self.max_adjustment_multiplier);
            
        ((base_reward as f64) * multiplier) as u64
    }
    
    /// Update multiplier (called periodically by validators)
    pub fn update_multiplier(&mut self, molt_price_usd: f64, current_slot: u64) {
        // Only update if enough time has passed
        if current_slot - self.last_update_slot < self.adjustment_frequency_slots {
            return;
        }
        
        let new_multiplier = (self.reference_price_usd / molt_price_usd)
            .max(self.min_adjustment_multiplier)
            .min(self.max_adjustment_multiplier);
            
        self.current_multiplier = new_multiplier;
        self.last_update_slot = current_slot;
    }
    
    /// Get current transaction reward
    pub fn get_transaction_reward(&self, molt_price_usd: f64) -> u64 {
        self.calculate_adjusted_reward(self.base_transaction_reward, molt_price_usd)
    }
    
    /// Get current heartbeat reward
    pub fn get_heartbeat_reward(&self, molt_price_usd: f64) -> u64 {
        self.calculate_adjusted_reward(self.base_heartbeat_reward, molt_price_usd)
    }
}
```

### 2. Price Oracle Interface

```rust
// core/src/oracle.rs (new file)

/// Simple price oracle interface
pub trait PriceOracle {
    /// Get current MOLT/USD price
    fn get_molt_price_usd(&self) -> Result<f64, String>;
    
    /// Check if price is stale
    fn is_stale(&self) -> bool;
}

/// Mock oracle for testnet (always returns reference price)
pub struct MockOracle {
    price: f64,
}

impl MockOracle {
    pub fn new(price: f64) -> Self {
        MockOracle { price }
    }
}

impl PriceOracle for MockOracle {
    fn get_molt_price_usd(&self) -> Result<f64, String> {
        Ok(self.price)
    }
    
    fn is_stale(&self) -> bool {
        false
    }
}

/// On-chain oracle (reads from oracle account)
pub struct OnChainOracle {
    oracle_account: Pubkey,
    max_staleness_slots: u64,
}

impl OnChainOracle {
    pub fn new(oracle_account: Pubkey) -> Self {
        OnChainOracle {
            oracle_account,
            max_staleness_slots: 100, // ~40 seconds
        }
    }
}

impl PriceOracle for OnChainOracle {
    fn get_molt_price_usd(&self) -> Result<f64, String> {
        // TODO: Read from oracle account in state
        // For now, return fallback
        Ok(1.0)
    }
    
    fn is_stale(&self) -> bool {
        // TODO: Check last update timestamp
        false
    }
}
```

### 3. Integration with StakePool

```rust
// Update StakePool to use RewardConfig

impl StakePool {
    pub fn distribute_block_reward_with_price(
        &mut self,
        validator: &Pubkey,
        slot: u64,
        is_heartbeat: bool,
        molt_price_usd: f64,
        reward_config: &RewardConfig,
    ) -> u64 {
        if let Some(stake_info) = self.stakes.get_mut(validator) {
            if stake_info.is_active {
                let reward = if is_heartbeat {
                    reward_config.get_heartbeat_reward(molt_price_usd)
                } else {
                    reward_config.get_transaction_reward(molt_price_usd)
                };
                
                stake_info.add_reward(reward, slot);
                return reward;
            }
        }
        0
    }
}
```

### 4. Validator Integration

```rust
// validator/src/main.rs

// Add RewardConfig to validator state
let mut reward_config = RewardConfig::new();
let mock_oracle = MockOracle::new(1.0); // Testnet: fixed $1.00

// When distributing rewards:
let molt_price = mock_oracle.get_molt_price_usd().unwrap_or(1.0);
let reward = stake_pool.distribute_block_reward_with_price(
    &validator_pubkey,
    slot,
    is_heartbeat,
    molt_price,
    &reward_config,
);
```

## Implementation Status

**Phase 1: Fixed Rewards (CURRENT - LIVE)**
```rust
const TRANSACTION_BLOCK_REWARD: u64 = 100_000_000; // 0.1 MOLT
const HEARTBEAT_BLOCK_REWARD: u64 = 50_000_000;    // 0.05 MOLT
// No price adjustment
```

**Phase 2: Price-Adjusted Rewards (IMPLEMENT NOW)**
```rust
RewardConfig {
    base: 0.1 MOLT,
    reference_price: $1.00,
    current_multiplier: (reference / actual_price),
    bounds: [0.1x, 10x],
}
// Automatic adjustment based on $MOLT price
```

**Phase 3: On-Chain Oracle (FUTURE - Month 4+)**
```rust
// Read MOLT price from on-chain oracle account
// Update every epoch (~24h)
// Validators vote on new multiplier
// Governance can override
```

## Example Scenarios

### Scenario 1: Low Price ($MOLT = $0.01)
```
Base reward: 0.1 MOLT
Reference price: $1.00
Current price: $0.01
Multiplier: 1.00 / 0.01 = 100x

Adjusted reward: 0.1 * 100 = 10 MOLT
USD value: 10 × $0.01 = $0.10 ✓

Result: Validator earns same $0.10 USD worth
```

### Scenario 2: High Price ($MOLT = $100)
```
Base reward: 0.1 MOLT
Reference price: $1.00
Current price: $100
Multiplier: 1.00 / 100 = 0.01x

Adjusted reward: 0.1 * 0.01 = 0.001 MOLT
USD value: 0.001 × $100 = $0.10 ✓

Result: Validator earns same $0.10 USD worth
```

### Scenario 3: Extreme Price ($MOLT = $10,000)
```
Base reward: 0.1 MOLT
Reference price: $1.00
Current price: $10,000
Calculated multiplier: 1.00 / 10,000 = 0.0001x
Capped multiplier: max(0.0001, 0.1) = 0.1x (min bound)

Adjusted reward: 0.1 * 0.1 = 0.01 MOLT
USD value: 0.01 × $10,000 = $100 (10x intended)

Result: Validator earns 10x intended due to safety cap
Note: Governance would update reference_price in this scenario
```

## Economic Benefits

**For Validators:**
- Predictable USD-equivalent income
- Not affected by short-term price volatility
- Sustainable long-term participation

**For Network:**
- Validator profitability remains constant
- No exodus during price crashes
- No overpayment during price surges
- Sustainable token emissions

**Comparison to Fixed Rewards:**
```
$MOLT Price | Fixed (0.1 MOLT) | Adjusted (target $0.10 USD)
---------   | ------------------| --------------------------
$0.01       | $0.001/block      | 10 MOLT = $0.10/block ✓
$0.10       | $0.01/block       | 1.0 MOLT = $0.10/block ✓
$1.00       | $0.10/block ok    | 0.1 MOLT = $0.10/block ✓
$10.00      | $1.00/block $$    | 0.01 MOLT = $0.10/block ✓
$100.00     | $10/block $$$     | 0.001 MOLT = $0.10/block ✓
```

## Governance Parameters

**Adjustable via DAO vote:**
- `reference_price_usd` - Update when long-term price changes
- `target_reward_usd` - Increase/decrease validator profitability
- `max/min_adjustment_multiplier` - Widen/narrow safety bounds
- `adjustment_frequency_slots` - More/less frequent updates

**Example governance decision:**
```
Proposal: Increase validator rewards by 50%
Current: $0.10 USD per transaction block
Proposed: $0.15 USD per transaction block

Implementation: Update target_reward_usd from 0.10 to 0.15
Effect: All validators earn 50% more (in USD terms)
Token impact: 50% more MOLT emissions (inflation increase)
```

## Implementation Checklist

**Core Changes:**
- [ ] Add `RewardConfig` struct to consensus.rs
- [ ] Implement `calculate_adjusted_reward()` method
- [ ] Add `PriceOracle` trait (oracle.rs)
- [ ] Create `MockOracle` for testnet
- [ ] Update `StakePool::distribute_block_reward()` signature

**Validator Changes:**
- [ ] Initialize `RewardConfig` in main.rs
- [ ] Integrate `MockOracle` (testnet: $1.00 fixed)
- [ ] Update reward distribution calls
- [ ] Add logging for adjusted rewards

**Testing:**
- [ ] Unit tests for reward calculation
- [ ] Test with various price points ($0.01, $1, $100)
- [ ] Test boundary conditions (0.1x, 10x caps)
- [ ] Integration test with full validator

**Documentation:**
- [x] This design doc
- [ ] Update ECONOMICS.md with reward adjustment section
- [ ] Add to validator operator guide

**Future (Phase 3):**
- [ ] On-chain oracle account structure
- [ ] Oracle update transaction type
- [ ] Price feed aggregation (multiple sources)
- [ ] Governance voting on reference_price updates

## Rollout Plan

**Week 1 (Now):**
1. Implement RewardConfig
2. Add MockOracle
3. Update StakePool
4. Deploy to testnet

**Week 2-3:**
1. Monitor testnet behavior
2. Collect validator feedback
3. Adjust parameters if needed

**Month 2:**
1. Activate on mainnet with MockOracle ($1.00 fixed)
2. Manual governance updates to reference_price as needed

**Month 4+ (after DEX launch):**
1. Implement on-chain oracle
2. Read MOLT price from ClawSwap AMM
3. Automatic adjustment every epoch
4. Full decentralization

---

**Status**: Design complete, ready to implement  
**Complexity**: Medium (similar to fee adjustment)  
**Timeline**: 1-2 weeks to production  
**Dependencies**: None (can deploy independently)
