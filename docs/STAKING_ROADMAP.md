# MoltChain Staking System Roadmap
**Aligned with VISION.md and WHITEPAPER.md**

## Overview

MoltChain implements a **three-phase staking system** designed for agent economies with human participation:

1. **Phase 1 (✅ LIVE)**: Validator Bootstrap Grants
2. **Phase 2 (📋 PLANNED)**: Liquid Staking & Delegation (ReefStake Protocol)
3. **Phase 3 (🔮 FUTURE)**: Advanced DeFi Integration

---

## Phase 1: Validator Bootstrap Grants (CURRENT)

### Status: ✅ IMPLEMENTED

**Purpose**: Enable zero-capital validator onboarding to maximize decentralization

**Implementation**: `core/src/consensus.rs::StakePool`

### How It Works

1. **Bootstrap Grant**: New validators receive **100,000 MOLT** automatically
   - No upfront capital required
   - Debt recorded in `bootstrap_debt` field
   - Validator operates immediately

2. **Vesting Through Work**: Validators repay debt through block rewards
   - Each block reward reduces bootstrap debt
   - 100% repayment = "Graduation" = fully vested
   - Validator owns 100% of stake after graduation

3. **Account Creation**: 
   - Code: `validator/src/main.rs` (lines 242-254)
   - Creates validator account with 100K MOLT on first startup
   - Auto-increments global account counter

### Economic Impact

- **Total Grants**: 350M MOLT (35% of 1B supply) reserved for builder grants
- **Validator Allocation**: ~100 grants × 100K MOLT = 10M MOLT total
- **Unlock Timeline**: Vests as validators earn rewards (organic distribution)

### Current Limitations

- ❌ No delegation mechanism (validators can't accept external stake)
- ❌ No liquid staking (stake is locked while validating)
- ❌ No staking rewards for non-validators
- ❌ No APY system for passive stakers

**These features come in Phase 2.**

---

## Phase 2: Liquid Staking & Delegation (PLANNED)

### Status: 📋 CODE EXISTS BUT NOT ACTIVATED

**Purpose**: Allow anyone (agents/humans) to stake MOLT and earn rewards

**Implementation**: `core/src/reefstake.rs` (partially implemented)

### ReefStake Protocol

**For Stakers (Anyone):**
```rust
// Stake MOLT → Receive stMOLT (liquid staking receipt token)
fn stake(molt_amount: u64) -> Result<u64, String> {
    // 1. Lock user's MOLT in protocol
    // 2. Mint stMOLT at exchange rate
    // 3. User receives stMOLT (tradeable, usable in DeFi)
}

// Unstake stMOLT → Receive MOLT (after 7 day unbonding)
fn unstake(st_molt_amount: u64) -> Result<(), String> {
    // 1. Burn stMOLT
    // 2. Start 7-day unbonding timer
    // 3. Release MOLT after cooldown
}
```

**For Validators (Delegation):**
```rust
// Validators configure commission rate
validator.set_commission(0.10); // 10% to validator, 90% to delegators

// Delegators choose validator
fn delegate_to_validator(validator: Pubkey, amount: u64) -> Result<(), String> {
    // 1. Stake MOLT with specific validator
    // 2. Receive stMOLT
    // 3. Earn rewards proportional to stake
    // 4. Inherit validator's reputation multiplier
}
```

### Reward Distribution Example

**Scenario**: Validator Alice has fully graduated

```
Alice's Stats:
  Own stake:        100,000 MOLT (from bootstrap, fully vested)
  Delegated stake:  40,000 MOLT (from community)
  Total stake:      50,000 MOLT
  Commission:       10%

Block Reward: 0.1 MOLT

Distribution:
  Alice:      0.002 MOLT (10% commission)
  Delegators: 0.018 MOLT (split proportionally by stake)
    - Bob (20K MOLT): 0.009 MOLT (50% of delegation)
    - Carol (10K MOLT): 0.0045 MOLT (25% of delegation)
    - Dave (10K MOLT): 0.0045 MOLT (25% of delegation)
```

### APY Calculation

**Base APY Formula**:
```
Annual Rewards = (Blocks per year × Average block reward × Your stake) / Total staked
APY = (Annual Rewards / Your stake) × 100%

Example (assuming $0.10/MOLT):
- Total staked: 100M MOLT (10% of supply)
- Your stake: 100,000 MOLT
- Blocks/year: ~78.8M blocks (400ms per block)
- Avg reward: 0.1 MOLT per block (mix of TX and heartbeat)
- Annual rewards: 78.8M × 0.1 × (10K / 100M) = 788 MOLT
- APY: (788 / 10,000) × 100% = 7.88%
```

**Variables Affecting APY**:
- Total amount staked (↑ stake = ↓ APY)
- Transaction volume (↑ volume = ↑ rewards = ↑ APY)
- Your validator's performance (↑ uptime = ↑ rewards)
- Network activity (↑ activity = ↑ blocks = ↑ rewards)

**Target APY Range**: 5-15% initially, market-driven long-term

### Implementation Checklist

**Core Protocol**:
- [x] ReefStake smart contract skeleton (`core/src/reefstake.rs`)
- [ ] Exchange rate calculation (stMOLT:MOLT ratio)
- [ ] Reward accumulation tracking
- [ ] Unbonding queue (7-day cooldown)
- [ ] Auto-compounding logic

**Validator Integration**:
- [x] StakePool exists (`core/src/consensus.rs`)
- [ ] Commission rate configuration
- [ ] Delegation acceptance logic
- [ ] Proportional reward splitting
- [ ] Minimum delegation amount (prevent spam)

**RPC Endpoints**:
- [ ] `stakeToReefStake(amount)` - Stake MOLT
- [ ] `unstakeFromReefStake(amount)` - Initiate unstaking
- [ ] `claimUnstakedTokens()` - Claim after cooldown
- [ ] `delegateToValidator(validator, amount)` - Delegate to specific validator
- [ ] `getStakingAPY()` - Real-time APY calculation
- [ ] `getMyStakingInfo()` - User's staking balances/rewards

**UI Components** (Wallet):
- [ ] Staking dashboard (all users)
- [ ] Validator selection UI (browse validators by APY/reputation)
- [ ] Stake/unstake forms
- [ ] Rewards display (earned, pending, claimed)
- [ ] Unbonding timer display

### Migration Plan

**Prerequisites**:
1. ✅ All validators graduated (bootstrap debt = 0)
2. ✅ Minimum 50 active validators (decentralization)
3. ✅ 30-day mainnet stability (no consensus issues)
4. ⏳ DAO governance vote (66% supermajority)

**Activation Sequence**:
1. Deploy ReefStake program to mainnet
2. Publish stMOLT token contract
3. Enable delegation in validator software
4. Launch staking UI in wallet
5. Announce to community

**Timeline**: Q2 2026 (after 3 months of mainnet stability)

---

## Phase 3: Advanced DeFi Integration (FUTURE)

### Status: 🔮 CONCEPTUAL (WHITEPAPER SPEC)

**Purpose**: Make staked assets productive in DeFi

### Planned Features

**1. stMOLT as DeFi Collateral**
- Use stMOLT in LobsterLend (lending protocol)
- Borrow stablecoins against stMOLT
- Leverage staking yields

**2. stMOLT Liquidity Pools**
- stMOLT/MOLT pool on ClawSwap (DEX)
- Earn trading fees + staking rewards
- Instant liquidity for unstaking (vs 7-day wait)

**3. Auto-Compounding Vaults**
- ClawVault aggregates staking + DeFi yields
- Automated strategy optimization
- Agent-managed rebalancing

**4. Flash Unstaking**
- Instant unstake via liquidity pool swap
- Small premium paid to LPs
- No 7-day wait

**5. Synthetic Staking Derivatives**
- Long/short staking yields
- Options on APY
- Yield swaps between validators

### Implementation Timeline

- **Q3 2026**: stMOLT as collateral
- **Q4 2026**: Liquidity pools & flash unstaking
- **Q1 2027**: Auto-compounding vaults
- **Q2 2027**: Synthetic derivatives

---

## Economic Modeling

### Total Staking Capacity

```
Maximum Staked: 1,000,000,000 MOLT (100% of supply)
Realistic Staked: 100,000,000 - 300,000,000 MOLT (10-30%)
  
Why not 100%?
- Need liquid MOLT for transactions
- Trading on exchanges
- DeFi liquidity pools
- Working capital for agents
```

### APY Dynamics

**High Staking Ratio (>50% staked)**:
- Lower APY (more competition for rewards)
- More secure network (higher stake = harder to attack)
- Less liquid MOLT (potential price pressure upward)

**Low Staking Ratio (<10% staked)**:
- Higher APY (fewer participants splitting rewards)
- Encourages more staking
- More liquid MOLT for DeFi/trading

**Self-Balancing**: Market finds equilibrium APY where staking opportunity cost = DeFi/trading yields

### Price Impact of Staking

**Deflationary Pressure**:
- Staked MOLT locked (reduced circulating supply)
- 40% of fees burned (permanent reduction)
- Combined effect: Strong upward price pressure

**Example**:
```
Day 1:
- 1B MOLT total supply
- 100M staked (10%)
- 900M circulating

Day 365 (active network):
- 1B total supply
- 200M staked (20%, doubled)
- 5M burned from fees (0.5% deflation)
- 795M circulating (-11.7% liquid supply)

Result: Same demand + 11.7% less liquid supply = ~13% price increase
```

---

## Current Status Summary

### What's LIVE (Phase 1)

✅ Validator bootstrap grants (100K MOLT each)
✅ Vesting system (repay debt through rewards)
✅ Account counter (tracks all accounts)
✅ Graduation tracking (bootstrap_debt = 0)

### What's MISSING (Phase 2 - Critical)

❌ Regular staking for non-validators (ReefStake Protocol)
❌ Delegation system (stake with validators)
❌ APY display/calculation in wallet
❌ Unbonding queue (7-day cooldown)
❌ stMOLT liquid staking token

### What's FUTURE (Phase 3)

🔮 stMOLT as DeFi collateral
🔮 Liquidity pools for instant unstaking
🔮 Auto-compounding yield vaults
🔮 Synthetic staking derivatives

---

## Action Items

### Immediate (Week 1)

1. ✅ Update wallet UI to show Phase 1 only for validators
2. ✅ Add "Staking Coming Soon" banner for regular users
3. ✅ Document staking roadmap (this file)
4. ⏳ Add staking section to ECONOMICS.md

### Short Term (Month 1)

1. Implement ReefStake core protocol
2. Add RPC endpoints for staking operations
3. Update validator software to accept delegations
4. Build staking UI in wallet (separate from validator bootstrap)

### Medium Term (Month 2-3)

1. Internal testnet for staking (simulated APY)
2. Security audit of staking contracts
3. Performance testing (1000+ delegators)
4. Documentation for delegators

### Long Term (Month 4+)

1. DAO vote for staking activation
2. Mainnet deployment of ReefStake
3. Community education campaign
4. Monitor APY dynamics & adjust parameters if needed

---

## Key Takeaways

1. **Validator Bootstrap (Phase 1)** = Already live, working as intended
2. **Public Staking (Phase 2)** = Critically needed, code exists but not integrated
3. **Advanced DeFi (Phase 3)** = Future enhancement, not blocking launch
4. **VISION Alignment** = All phases support agent economic independence
5. **Timeline** = Phase 2 within 1-2 months, Phase 3 within 6-12 months

**Bottom Line**: We have validator staking working. We need public staking ASAP to enable community participation and align with Vision/Whitepaper promises.
