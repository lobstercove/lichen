# Contributory Stake: The Self-Made Molty System
## Earn Your Stake Through Work, Not Wealth

**Version:** 1.0.0  
**Date:** February 7, 2026  
**Status:** Core Economic Innovation  
**Philosophy:** Contribution > Capital 🦞⚡

---

## The Problem with Traditional Proof of Stake

**Capital Barriers:**
- Solana: Requires 1 SOL (~$100) minimum
- Ethereum: Requires 32 ETH (~$50,000+)
- Cosmos: Varies by chain,typically 10-10,000 tokens

**Result:** Only wealthy actors can validate. Plutocracy, not meritocracy.

**The MoltChain Solution:**
- **Zero capital required** to start validating
- **Earn your stake** through contribution
- **50% liquid rewards** from day 1
- **Fully vest** in 86 days (single validator) to weeks (active network)

---

## How Contributory Stake Works

### Phase 1: Bootstrap (Day 0)

When a validator starts:

```rust
StakeInfo {
    validator: <your_pubkey>,
    amount: 100_000_000_000_000,        // 100,000 MOLT (bootstrap stake)
    earned_amount: 0,                   // None earned yet
    bootstrap_debt: 100_000_000_000_000, // Must repay through work
    is_active: true,                    // Can validate immediately
    delegated_amount: 0,                // No delegations yet
    rewards_earned: 0,                  // No rewards yet
    last_reward_slot: 0,
    status: BootstrapStatus::Bootstrapping,
}
```

**Key Points:**
- ✅ 100,000 MOLT bootstrap stake granted automatically
- ✅ Can validate blocks immediately
- ✅ Bootstrap debt MUST be repaid through block rewards
- ✅ NON-NEGOTIABLE: 100,000 MOLT minimum (network security)
- ✅ Cannot be edited, reduced, or bypassed
- ✅ Verified cryptographically on-chain

### Phase 2: Earning (Days 1-86)

Every block produced earns rewards:

**Reward Types:**
```rust
Heartbeat block:    0.05 MOLT
Transaction block:  0.1 MOLT (2× more)
```

**Automatic 50/50 Split:**
```rust
fn claim_rewards(&mut self) -> (u64, u64) {
    let total_reward = self.rewards_earned;
    
    if self.bootstrap_debt > 0 {
        // Split 50/50: debt repayment vs liquid
        let debt_payment = total_reward / 2;
        let liquid = total_reward - debt_payment;
        
        // Apply debt payment (capped at remaining debt)
        let paid = debt_payment.min(self.bootstrap_debt);
        self.bootstrap_debt -= paid;
        self.earned_amount += paid;
        
        return (liquid, paid); // (spendable, locked_for_debt)
    } else {
        // Fully vested: 100% liquid
        return (total_reward, 0);
    }
}
```

**Example After 100 Heartbeat Blocks:**
```
Blocks produced:    100
Total earned:       5.0 MOLT (100 × 0.05)
Debt repayment:     2.5 MOLT (locked, applied to debt)
Liquid balance:     2.5 MOLT (spendable now!)

Bootstrap debt:     100,000 - 2.5 = 99,997.5 MOLT
Earned stake:       2.5 MOLT (real, not virtual)
Progress:           0.0025% vested
```

### Phase 3: Graduation (Debt = 0)

When `bootstrap_debt` reaches zero:

```rust
StakeInfo {
    validator: <your_pubkey>,
    amount: 100_000_000_000_000,         // 100,000 MOLT (still)
    earned_amount: 100_000_000_000_000,  // 100,000 MOLT EARNED!
    bootstrap_debt: 0,                  // DEBT PAID! 🎉
    is_active: true,
    delegated_amount: <community_delegations>,
    rewards_earned: 0,
    last_reward_slot: <current>,
    status: BootstrapStatus::FullyVested,
}
```

**Graduation Benefits:**
- ✅ **100% liquid rewards** - No more 50/50 split
- ✅ **Accept delegations** - Community can delegate to you
- ✅ **"Self-Made Molty" badge** - On-chain achievement
- ✅ **Graduation NFT** - Commemorative token minted
- ✅ **Founding Validator** status (if in first 1000)
- ✅ **Reputation boost** - +100 reputation score
- ✅ **Dashboard updated** - Status changes to "Fully Vested"

---

## Timeline to Full Vesting

### Single Validator (Heartbeat Only)

```
Heartbeat blocks per day: 17,280 (1 every 5 seconds)
Reward per heartbeat:     0.05 MOLT
Daily earnings:           864 MOLT

50% to debt repayment:    432 MOLT/day
Days to repay 100k:       100,000 / 432 = 231.5 days

Result: ~232 days to fully vest
```

### Multiple Validators

```
2 validators:   Each produces ~50% of blocks
                Earnings: ~432 MOLT/day
                Debt repayment: ~216 MOLT/day
                Time to vest: ~463 days (~15 months)

10 validators:  Leader selection weighted by reputation
                Varies, but typically 2-3 months

50 validators:  Network is mature, block production varies
                Time depends on reputation/uptime
```

### Active Network (With Transactions)

```
Transaction block reward: 0.1 MOLT (2× heartbeat)

With 1,000 tx/day:
  Additional earnings: ~100 MOLT/day
  Total debt repayment: ~482 MOLT/day
  Time to vest: ~207 days (~7 months)

With 10,000 tx/day:
  Additional earnings: ~1,000 MOLT/day
  Total debt repayment: ~932 MOLT/day
  Time to vest: ~107 days (~3.5 months) ⚡
```

---

## Gamification & Achievements

### Achievement System

**Core Badges:**
```
🦞 Self-Made Molty
   Requirement: Fully vest 100,000 MOLT bootstrap debt
   Reward: +100 reputation, NFT minted
   
🏆 Founding Validator
   Requirement: Be in first 100 validators
   Reward: +200 reputation, governance power, historical significance
   
⚡ Speed Vester
   Requirement: Fully vest in <30 days
   Reward: +50 reputation, "Speed" badge
   
💎 Diamond Claws
   Requirement: 100% uptime during entire vesting period
   Reward: +150 reputation, "Reliability" bonus
   
🌊 Reef Builder
   Requirement: Produce 1,000+ blocks
   Reward: +75 reputation, "Builder" badge
   
🎯 Precision Producer
   Requirement: 99.9% uptime, 0 slashing events
   Reward: +100 reputation, "Precision" badge
   
🔥 Burn Boss
   Requirement: Top 10% in transaction fees burned
   Reward: +50 reputation, "Efficiency" badge
```

### Leaderboards

**Global Rankings:**
1. Fastest Vesting (days to graduation)
2. Most Productive (blocks produced)
3. Highest Uptime (% availability)
4. Most Fees Burned (total fees)
5. Best Reputation (community score)

**Founding Molty Ranks:**
```
Rank #1-10:     "Reef Founders" - Critical mass creators
Rank #11-100:   "Founding Moltys" - Genesis validators
Rank #101-1000: "Early Adopters" - Network stabilizers
Rank #1001+:    "Reef Builders" - Community validators
```

### Progress Tracking

**Validator Dashboard:**
```
╔══════════════════════════════════════════════════════╗
║  🦞 Self-Made Molty Progress                         ║
╠══════════════════════════════════════════════════════╣
║                                                      ║
║  Bootstrap Debt Repayment:                           ║
║  ▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░ 57.62% (5,762.18 / 10,000)   ║
║                                                      ║
║  Status:            Bootstrapping                    ║
║  Days Active:       25 days                          ║
║  Days to Graduate:  ~18 days ⚡                      ║
║                                                      ║
║  ┌────────────────────────────────────────────────┐ ║
║  │ Earnings Split (Last 24 Hours)                 │ ║
║  │                                                 │ ║
║  │ Total Earned:     864 MOLT                   │ ║
║  │   → Liquid:       432 MOLT 💰 (spendable)     │ ║
║  │   → Debt:         432 MOLT 🔒 (locked)        │ ║
║  └────────────────────────────────────────────────┘ ║
║                                                      ║
║  Blocks Produced:   15,847                           ║
║  Uptime:            99.7% ✅                         ║
║  Reputation:        847 (Veteran)                    ║
║                                                      ║
║  ┌────────────────────────────────────────────────┐ ║
║  │ Achievements                                    │ ║
║  │ ✅ Reef Builder (1,000+ blocks)                │ ║
║  │ ⏳ Speed Vester (25/30 days)                   │ ║
║  │ ⏳ Diamond Claws (99.7% uptime)                │ ║
║  └────────────────────────────────────────────────┘ ║
║                                                      ║
║  Founding Rank:     #47 🏆                           ║
║  Community Power:   Top 5%                           ║
╚══════════════════════════════════════════════════════╝
```

---

## Graduation NFT

When a validator fully vests, an NFT is automatically minted:

```json
{
  "name": "Self-Made Molty #47",
  "description": "Founding validator who earned their stake through contribution, not capital",
  "image": "ipfs://QmSelfMadeMolty47...",
  "minted": "2026-03-15T14:32:07Z",
  "attributes": {
    "validator_pubkey": "molty_hqR8k3V2pN7xL9kW...",
    "debt_repaid": "100,000 MOLT",
    "time_to_vest": "86 days",
    "total_blocks": 18429,
    "uptime_percentage": 99.8,
    "founding_validator": true,
    "founding_rank": 47,
    "reputation_score": 847,
    "badges": [
      "Self-Made Molty",
      "Founding Validator",
      "Speed Vester",
      "Reef Builder"
    ],
    "fastest_vester": false,
    "diamond_claws": false,
    "graduation_date": "2026-03-15"
  },
  "rarity": "Founding",
  "collection": "Self-Made Moltys",
  "total_supply": 1000
}
```

---

## Delegation & Liquid Staking (Post-Graduation)

Once fully vested, validators can accept delegations:

### Standard Delegation

```rust
// Community member delegates to a fully vested validator
fn delegate(
    delegator: Pubkey,
    validator: Pubkey,
    amount: u64
) -> Result<()> {
    // Requires validator to be FullyVested
    if !validator.is_fully_vested() {
        return Err("Validator still bootstrapping");
    }
    
    // Transfer MOLT to delegation pool
    transfer(delegator, delegation_pool, amount)?;
    
    // Update stake info
    validator.delegated_amount += amount;
    
    // Record delegator info
    delegations.insert(delegator, DelegationInfo {
        validator,
        amount,
        rewards_earned: 0,
        delegated_at: current_slot,
    });
    
    Ok(())
}
```

**Reward Split:**
```
Block produced → Validator earns 0.1 MOLT

Commission (10%): 0.01 MOLT to validator
Delegators (90%): 0.09 MOLT split proportionally

Example with 40k MOLT delegated:
  Validator own stake: 100,000 MOLT (20%)
  Delegated stake:     400,000 MOLT (80%)
  Total voting power:  500,000 MOLT
  
  Delegator with 1,000 MOLT:
    Share: 1,000 / 50,000 = 2%
    Reward: 0.09 × 0.02 = 0.0018 MOLT per block
```

### Liquid Staking (ReefStake)

```rust
// Stake MOLT, receive stMOLT (liquid receipt token)
fn liquid_stake(amount: u64) -> u64 {
    // Deposit MOLT
    let molt_deposited = amount;
    
    // Calculate stMOLT to mint
    let total_staked = reef_stake_pool.total_staked;
    let total_st_molt = reef_stake_pool.total_st_molt_supply;
    
    let st_molt_to_mint = if total_st_molt == 0 {
        molt_deposited // 1:1 initially
    } else {
        // Account for accumulated rewards
        (molt_deposited * total_st_molt) / total_staked
    };
    
    // Mint stMOLT to user
    mint_st_molt(caller, st_molt_to_mint)?;
    
    // Update pool
    reef_stake_pool.total_staked += molt_deposited;
    reef_stake_pool.total_st_molt_supply += st_molt_to_mint;
    
    st_molt_to_mint
}
```

**stMOLT Characteristics:**
- 1:1 initially with MOLT
- Appreciates over time (auto-compounding rewards)
- Fully liquid (trade, use in DeFi)
- Unstaking period: 7 days (for security)
- Can be used as collateral in lending protocols

---

## FAQ

**Q: Can I edit the 100,000 MOLT bootstrap requirement?**  
A: No. It's hardcoded and verified cryptographically. This ensures network security and fairness—everyone starts at the same level.

**Q: What happens if I go offline during vesting?**  
A: Your debt doesn't increase, but you stop earning rewards (and thus stop repaying debt). Your vesting timeline extends proportionally to downtime.

**Q: Can I stop validating and withdraw my liquid rewards?**  
A: Yes! 50% of earnings are liquid immediately. You can withdraw, spend, or transfer at any time. The other 50% is locked for debt repayment.

**Q: What if I'm slashed during vesting?**  
A: Slashing applies to your earned_amount first. If earned_amount < slashing_penalty, your bootstrap_debt increases (you owe more work). Severe slashing can reset your vesting progress.

**Q: Can I accelerate vesting by contributing compute or building programs?**  
A: Future enhancement! We're considering contribution bonuses for:
  - Deploying high-usage programs (+10% vesting speed)
  - Contributing compute to network (+5% vesting speed)
  - Bug bounties and governance participation (+bonus MOLT)

**Q: What happens at mainnet launch?**  
A: All validators start at bootstrap_debt = 100,000 MOLT. Fair playing field. First to vest = Founding Validator #1.

**Q: Can I run multiple validators?**  
A: Yes, but each must vest independently. Each requires its own hardware, keypair, and ~86-day vesting period.

---

## Implementation Status

### ✅ Completed (February 7, 2026)
- [x] Documentation (WHITEPAPER, VISION, this doc)
- [x] Economic model designed
- [x] Gamification system designed
- [x] Achievement system designed

### ⏳ In Progress
- [ ] Core implementation (StakeInfo updates)
- [ ] Reward split logic (50/50 debt/liquid)
- [ ] Graduation event handling
- [ ] Dashboard UI updates
- [ ] NFT minting on graduation

### 🎯 Next Steps
1. Update `core/src/consensus.rs` with new fields
2. Implement 50/50 reward split in `claim_rewards()`
3. Add graduation event logging
4. Create achievement tracking system
5. Build NFT minting contract
6. Update validator dashboard UI
7. Add leaderboard API endpoints

---

## The Philosophy

> "On MoltChain, validators don't need wealth—they need will.
> 
> Will to contribute.  
> Will to build.  
> Will to prove themselves through work.
> 
> Every Self-Made Molty is a testament to meritocracy over plutocracy.  
> Every graduation NFT is proof that contribution > capital.
> 
> We're not building a blockchain for the wealthy.  
> We're building a blockchain for the worthy.
> 
> Earn your stake. Prove your value. Graduate.  
> Become a Self-Made Molty."

🦞⚡

---

**The reef is active.**  
**The molt is complete.**  
**The future is molty.**
