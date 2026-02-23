# Tokenomics Overhaul — Complete Implementation Plan

**Date:** February 22, 2026
**Status:** Ready for implementation
**Scope:** Block rewards, genesis split, fee split, reward decay, vesting, wallet wiring, DEX rewards

---

## Executive Summary

This plan replaces the current unsustainable tokenomics (treasury depletion in ~3.7 years) with a long-term viable system:

| Change | Old | New |
|--------|-----|-----|
| TX block reward | 0.9 MOLT | **0.1 MOLT** |
| Heartbeat reward | 0.135 MOLT | **0.05 MOLT** |
| Reward decay | None | **20% annual** |
| Fee split | 50/30/10/10 (burn/prod/voter/treas) | **40/30/10/10/10** (burn/prod/voter/validator_pool/community) |
| Genesis split | 15/40/25/10/5/5 | **10/25/35/10/10/10** |
| DEX rewards | 500K MOLT/month (unimplemented) | **100K MOLT/month from builder_grants** |
| Vesting fee inclusion | Fees bypass vesting | **Fees go through vesting split** |
| Wallet wiring | Only validator_rewards used | **All 6 wallets active and wired** |
| Genesis auto-fund | Manual script | **Automatic 10K MOLT from treasury** |

**Constraints:**
- Zero inflation — fixed 1B MOLT supply, mandatory
- No stubs, no placeholders, no TODOs
- Every change gets a test
- Commit and push after each task
- No regressions

---

## Current State Audit

### Constants (canonical sources)

| Constant | Current Value | File:Line |
|----------|--------------|-----------|
| `TRANSACTION_BLOCK_REWARD` | 900,000,000 shells (0.9 MOLT) | `core/src/consensus.rs:24` |
| `HEARTBEAT_BLOCK_REWARD` | 135,000,000 shells (0.135 MOLT) | `core/src/consensus.rs:27` |
| `BLOCK_REWARD` | = TRANSACTION_BLOCK_REWARD | `core/src/consensus.rs:30` |
| `ANNUAL_REWARD_RATE_BPS` | 500 (5%) | `core/src/consensus.rs:34` |
| `SLOTS_PER_YEAR` | 78,840,000 | `core/src/consensus.rs:37` |
| `BOOTSTRAP_GRANT_AMOUNT` | 100,000 × 1e9 shells | `core/src/consensus.rs:21` |
| `MIN_VALIDATOR_STAKE` | 75,000 × 1e9 shells | `core/src/consensus.rs:18` |
| `MAX_BOOTSTRAP_VALIDATORS` | 200 | `core/src/consensus.rs:44` |
| `REWARD_POOL_MOLT` | 150,000,000 | `validator/src/main.rs:63` |
| `REWARD_POOL_PER_MONTH` (DEX) | 500,000,000,000,000 (500K MOLT) | `contracts/dex_rewards/src/lib.rs:28` |
| Fee burn % | 50 | `core/src/processor.rs:122` |
| Fee producer % | 30 | `core/src/processor.rs:123` |
| Fee voters % | 10 | `core/src/processor.rs:124` |
| Fee treasury % | 10 | `core/src/processor.rs:125` |

### Genesis Distribution (canonical: `core/src/multisig.rs:69-75`)

| Role | Current Amount | Current % | Used in code? |
|------|---------------|-----------|--------------|
| `validator_rewards` | 150,000,000 | 15% | YES — treasury, block rewards, bootstraps, fees |
| `community_treasury` | 400,000,000 | 40% | NO — allocated, never spent |
| `builder_grants` | 250,000,000 | 25% | NO — supposed to fund DEX rewards, never wired |
| `founding_moltys` | 100,000,000 | 10% | NO — allocated, never touched |
| `ecosystem_partnerships` | 50,000,000 | 5% | NO — allocated, never touched |
| `reserve_pool` | 50,000,000 | 5% | NO — allocated, never touched |

### Fee Flow (current)

```
User pays fee → Payer.deduct_spendable(fee)
                 ↓
         burn_amount (50%) → permanently burned (add_burned counter)
         remainder (50%) → treasury (validator_rewards wallet)
                 ↓
    [Block-level distribution in validator/src/main.rs]
         producer_share (30%) → producer_account.add_spendable() [BYPASSES VESTING]
         voters_share (10%) → voter_accounts.add_spendable() each
         treasury_share (10%) → stays in validator_rewards (not debited out)
```

### Block Reward Flow (current)

```
Block produced → pool.distribute_block_reward(producer, slot, is_heartbeat)
                 → stake_info.add_reward(reward, slot)
                 → pool.claim_rewards(producer, slot)
                    ↓
              [Vesting split in consensus.rs claim_rewards()]
              if bootstrap_debt > 0:
                  50% → debt repayment (earned_amount++)
                  50% → liquid (returned to caller)
                  [with 95%+ uptime: 75% debt / 25% liquid]
              else:
                  100% → liquid
                 ↓
           treasury.deduct_spendable(liquid)
           producer.add_spendable(liquid)
```

**Problem:** Fee producer_share goes directly to `add_spendable()`, completely bypassing the vesting pipeline. User wants fees to also split through vesting like block rewards.

---

## Implementation Tasks (Ordered)

### Task 1: Update Block Reward Constants

**Goal:** Change TX block reward from 0.9 → 0.1 MOLT, heartbeat from 0.135 → 0.05 MOLT.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 1a | `core/src/consensus.rs` | L24 | `TRANSACTION_BLOCK_REWARD: 900_000_000` → `100_000_000` |
| 1b | `core/src/consensus.rs` | L27 | `HEARTBEAT_BLOCK_REWARD: 135_000_000` → `50_000_000` |
| 1c | `core/src/consensus.rs` | L172 | `base_transaction_reward: TRANSACTION_BLOCK_REWARD` — no change needed (uses constant) |
| 1d | `core/src/consensus.rs` | L173 | `base_heartbeat_reward: HEARTBEAT_BLOCK_REWARD` — no change needed (uses constant) |
| 1e | `core/src/genesis.rs` | L93 | `validator_reward_per_block: 900_000_000` → `100_000_000` |
| 1f | `core/src/genesis.rs` | L392 | `validator_reward_per_block: 900_000_000` → `100_000_000` |
| 1g | `core/src/genesis.rs` | L439 | `validator_reward_per_block: 900_000_000` → `100_000_000` |

**Tests to update:**
| # | File | Line(s) | Change |
|---|------|---------|--------|
| 1h | `core/tests/production_readiness.rs` | L1253-1266 | Update expected reward values |

**New test:** `test_block_reward_values` — assert both constants match expected values.

---

### Task 2: Implement Reward Decay (20% Annual)

**Goal:** Block rewards decrease by 20% per year since genesis, computed deterministically from genesis slot.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 2a | `core/src/consensus.rs` | New constant | Add `pub const ANNUAL_REWARD_DECAY_BPS: u64 = 2000; // 20% annual decay` |
| 2b | `core/src/consensus.rs` | New function | Add `pub fn decayed_reward(base_reward: u64, slots_since_genesis: u64) -> u64` |
| 2c | `core/src/consensus.rs` | L1011-1029 `distribute_block_reward()` | Apply decay: `let reward = decayed_reward(base, slots_since_genesis)` |
| 2d | `core/src/consensus.rs` | Add `genesis_slot: u64` field to `StakePool` | Track genesis start for decay calculation |
| 2e | `core/src/consensus.rs` | `StakePool::new()` | Accept `genesis_slot` parameter |
| 2f | `validator/src/main.rs` | Where StakePool is created | Pass genesis slot 0 (or actual genesis slot from state) |
| 2g | `core/src/reefstake.rs` | L583-595 `calculate_apy_bp()` | Factor in decay for APY display |

**Decay function spec:**
```rust
pub fn decayed_reward(base_reward: u64, slots_since_genesis: u64) -> u64 {
    let years = slots_since_genesis / SLOTS_PER_YEAR;
    let mut reward = base_reward;
    // 20% decay per year → multiply by 80/100 each year
    // Cap iterations at 50 (reward is effectively 0 by then)
    for _ in 0..years.min(50) {
        reward = reward * 80 / 100;
    }
    reward
}
```

**Math verification:**
- Year 0: 0.1 MOLT / 0.05 MOLT
- Year 1: 0.08 / 0.04
- Year 5: 0.033 / 0.016
- Year 10: 0.011 / 0.005
- Year 20: 0.001 / 0.0005
- Year 50: ~0 / ~0

**New tests:**
- `test_decayed_reward_year_0` — base reward unchanged
- `test_decayed_reward_year_1` — 80% of base
- `test_decayed_reward_year_5` — expected value
- `test_decayed_reward_year_50` — near zero
- `test_decayed_reward_overflow_safe` — no panic with large slot numbers

---

### Task 3: Update Genesis Distribution

**Goal:** Change from 15/40/25/10/5/5 to 10/25/35/10/10/10.

**New distribution:**

| Role | New Amount | New % | Purpose |
|------|-----------|-------|---------|
| `validator_rewards` | 100,000,000 | 10% | Block rewards, bootstraps, fee recycling |
| `community_treasury` | 250,000,000 | 25% | DAO governance spending |
| `builder_grants` | 350,000,000 | 35% | DEX rewards, dev incentives, ecosystem growth |
| `founding_moltys` | 100,000,000 | 10% | Team/founders, 2-year vest |
| `ecosystem_partnerships` | 100,000,000 | 10% | Bridges, integrations, partnerships |
| `reserve_pool` | 100,000,000 | 10% | Emergency reserve, protocol insurance |

**Total: 1,000,000,000 MOLT (100%) ✓**

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 3a | `core/src/multisig.rs` | L69-75 | Update `GENESIS_DISTRIBUTION` array |
| 3b | `core/src/genesis.rs` | L337-370 `generate_genesis_distribution()` | Update amounts: 250M, 350M, 100M, 100M, 100M, 100M |
| 3c | `core/src/genesis.rs` | L506-508 | Update test assertions |
| 3d | `core/src/multisig.rs` | L306-308 | Update test assertions |
| 3e | `core/tests/caller_verification.rs` | L351-356 | Update `a12_01_genesis_distribution_matches_multisig()` |
| 3f | `validator/src/main.rs` | L63 | `REWARD_POOL_MOLT: 150_000_000` → `100_000_000` |
| 3g | `validator/src/main.rs` | L5434 | Update `reward_pool_molt` usage |
| 3h | `validator/src/main.rs` | L5545 | Update `reward_pool_molt` usage |
| 3i | `validator/src/main.rs` | L5645 | Update `reward_shells` usage |

**New test:** `test_genesis_distribution_sums_to_1b` — assert all 6 wallets sum to exactly 1,000,000,000.

---

### Task 4: Store All Wallet Pubkeys in State

**Goal:** Currently only `validator_rewards` pubkey is stored (as "treasury_pubkey"). Store ALL 6 wallet pubkeys so they can be identified and used.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 4a | `core/src/state.rs` | After `set_treasury_pubkey` | Add `set_community_treasury_pubkey()`, `get_community_treasury_pubkey()` |
| 4b | `core/src/state.rs` | After above | Add `set_builder_grants_pubkey()`, `get_builder_grants_pubkey()` |
| 4c | `core/src/state.rs` | After above | Add `set_founding_moltys_pubkey()`, `get_founding_moltys_pubkey()` |
| 4d | `core/src/state.rs` | After above | Add `set_ecosystem_partnerships_pubkey()`, `get_ecosystem_partnerships_pubkey()` |
| 4e | `core/src/state.rs` | After above | Add `set_reserve_pool_pubkey()`, `get_reserve_pool_pubkey()` |
| 4f | `validator/src/main.rs` | L5389-5391 (genesis distribution loop) | Store ALL wallet pubkeys during genesis, not just validator_rewards |

**State keys:**
- `b"treasury_pubkey"` → validator_rewards (already exists)
- `b"community_treasury_pubkey"` → community_treasury (NEW)
- `b"builder_grants_pubkey"` → builder_grants (NEW)
- `b"founding_moltys_pubkey"` → founding_moltys (NEW)
- `b"ecosystem_partnerships_pubkey"` → ecosystem_partnerships (NEW)
- `b"reserve_pool_pubkey"` → reserve_pool (NEW)

**New test:** `test_all_wallet_pubkeys_stored` — verify all 6 pubkeys are persisted and retrievable.

---

### Task 5: Update Fee Split (40/30/10/10/10)

**Goal:** Change to 40% burn, 30% producer, 10% voters, 10% validator_rewards (fee recycling), 10% community_treasury.

This splits the old `fee_treasury_percent` into two destinations and adds a new field.

**New fee flow:**
```
User pays fee → Payer.deduct_spendable(fee)
                 ↓
         burn (40%) → permanently burned
         remainder (60%) → treasury (validator_rewards) [temporary holding]
                 ↓
    [Block-level distribution]
         producer_share (30%) → vesting pipeline (NEW: goes through debt repay)
         voters_share (10%) → voter_accounts.add_spendable()
         validator_pool_share (10%) → stays in validator_rewards (fee recycling)
         community_share (10%) → community_treasury wallet (NEW)
```

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 5a | `core/src/processor.rs` | L98-113 `FeeConfig` struct | Add `pub fee_community_percent: u64` field |
| 5b | `core/src/processor.rs` | L116-128 `default_from_constants()` | `fee_burn_percent: 40`, `fee_treasury_percent: 10`, add `fee_community_percent: 10` |
| 5c | `core/src/processor.rs` | L1457-1500 `charge_fee_direct()` | Include `fee_community_percent` in `allocated` calculation |
| 5d | `core/src/genesis.rs` | L153-161 `GenesisFeatures` | Add `fee_community_percentage` field |
| 5e | `core/src/genesis.rs` | L182-185 defaults | Add `default_fee_community_percentage() -> u64 { 10 }` |
| 5f | `core/src/genesis.rs` | L256-275 validation | Update total pct validation to include community |
| 5g | `core/src/genesis.rs` | L415-417 | `fee_burn_percentage: 40`, add `fee_community_percentage: 10` |
| 5h | `core/src/genesis.rs` | L460-462 | Same update for mainnet config |
| 5i | `core/src/state.rs` | L4094-4110 `store_fee_config()` | Store `fee_community_percent` |
| 5j | `core/src/state.rs` | L4150-4156 `get_fee_config()` | Load `fee_community_percent` |
| 5k | `validator/src/main.rs` | L2436-2438 fee splitting | Compute `community_share`, transfer to community_treasury wallet |
| 5l | `validator/src/main.rs` | L2554-2567 treasury accounting | Debit community_share from treasury, credit to community_treasury |
| 5m | `validator/src/main.rs` | L5306-5318 genesis fee config | Include `fee_community_percent` in genesis setup |
| 5n | `rpc/src/lib.rs` | L2115-2140 `setFeeConfig` handler | Parse and validate `fee_community_percent` |
| 5o | `rpc/src/lib.rs` | L9073-9074 tokenomics endpoint | Return `fee_community_percent` |
| 5p | `scripts/generate-genesis.sh` | L270 | Update `fee_burn_percentage: 40`, add `fee_community_percentage: 10` |
| 5q | `core/tests/production_readiness.rs` | L617-620 | Update test fee config |
| 5r | `core/src/state.rs` | L5900-5903 | Update test fee config |

**New tests:**
- `test_fee_split_sums_to_100` — assert all 5 fields sum to exactly 100
- `test_community_treasury_receives_fees` — verify community_treasury wallet balance increases after tx

---

### Task 6: Route Producer Fee Share Through Vesting

**Goal:** Producer fee share (30%) must go through the same vesting pipeline as block rewards — 50/50 debt repay / liquid split while bootstrap_debt > 0.

**Current flow (BROKEN):**
```
producer_share → producer_account.add_spendable()  // BYPASSES VESTING
```

**New flow:**
```
producer_share → pool.add_fee_reward(producer, fee_share)
              → stake_info.add_reward(fee_share, slot)
              → pool.claim_rewards(producer, slot)
              → vesting split (50/50 or 75/25)
              → liquid portion credited to producer
```

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 6a | `core/src/consensus.rs` | After `distribute_block_reward()` | Add `pub fn distribute_fee_reward(&mut self, validator: &Pubkey, fee_amount: u64, slot: u64) -> u64` |
| 6b | `validator/src/main.rs` | L2450-2462 (producer_share crediting) | Replace `add_spendable(producer_share)` with `pool.distribute_fee_reward() + pool.claim_rewards()` |
| 6c | `validator/src/main.rs` | L2280-2305 (block reward crediting) | Ensure combined flow works: block reward + fee reward both go through claim_rewards |

**Important:** The fee distribution happens AFTER block reward distribution. Two possible approaches:

**Approach A (Combined):** In the block production flow, compute fee share BEFORE calling `claim_rewards()`, add both block_reward + fee_share to `rewards_earned`, then call `claim_rewards()` once.

**Approach B (Sequential):** Call `distribute_fee_reward()` + `claim_rewards()` a second time in the fee distribution function. This is simpler but means claim_rewards() is called twice per block for the producer.

**Chosen: Approach B** — less risky, no restructuring of the existing block reward flow. The second `claim_rewards()` call handles fee share with the same vesting logic.

**Implementation detail:**
```rust
// In distribute_fees() — validator/src/main.rs
if producer_share > 0 {
    let (fee_liquid, fee_debt_payment) = {
        let mut pool = stake_pool.write().await;
        pool.distribute_fee_reward(&producer, producer_share, slot);
        let (liquid, debt) = pool.claim_rewards(&producer, slot);
        let pool_snapshot = pool.clone();
        drop(pool);
        state.put_stake_pool(&pool_snapshot).ok();
        (liquid, debt)
    };
    
    // Debit treasury, credit producer (only liquid portion)
    if fee_liquid > 0 {
        let mut producer_account = state.get_account(&producer)...;
        producer_account.add_spendable(fee_liquid)...;
        // treasury debit handled in overall flow
    }
}
```

**New tests:**
- `test_fee_share_goes_through_vesting` — producer with bootstrap_debt receives only 50% of fee share as liquid
- `test_fee_share_fully_vested` — producer with no debt receives 100% of fee share

---

### Task 7: Wire Community Treasury Spending

**Goal:** The community_treasury wallet (250M MOLT) must be spendable via DAO governance. The `moltdao` contract has a `treasury_transfer` function — wire it to actually debit from the community_treasury wallet.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 7a | `contracts/moltdao/src/lib.rs` | L964 `treasury_transfer()` | Wire to use the community_treasury pubkey from state |
| 7b | `validator/src/main.rs` | Contract execution context | Make community_treasury_pubkey available to contract runtime |
| 7c | `rpc/src/lib.rs` | Tokenomics endpoint | Add community_treasury balance and pubkey to response |

**New test:** `test_dao_treasury_transfer_debits_community_treasury` — DAO proposal execution actually moves MOLT from community_treasury.

---

### Task 8: Wire Builder Grants as DEX Reward Source

**Goal:** DEX reward claims must debit from the `builder_grants` wallet (350M MOLT). Currently reward claims track bookkeeping but never transfer MOLT.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 8a | `contracts/dex_rewards/src/lib.rs` | L28 | `REWARD_POOL_PER_MONTH: 500K` → `100K` (100,000,000,000,000 shells) |
| 8b | `contracts/dex_rewards/src/lib.rs` | Reward claim function | Wire actual MOLT transfer from builder_grants wallet |
| 8c | `contracts/dex_rewards/src/lib.rs` | L1237-1239 | Update test assertion to 100K |
| 8d | `validator/src/main.rs` | Contract execution context | Make builder_grants_pubkey available to contract runtime |

**New test:** `test_dex_reward_claim_debits_builder_grants` — claiming DEX rewards actually transfers from builder_grants.

---

### Task 9: Auto-Fund Genesis/Deployer from Treasury

**Goal:** During genesis boot, automatically transfer 10K MOLT from `validator_rewards` to the genesis/deployer account. Eliminate the need for `scripts/fund-deployer.py`.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 9a | `validator/src/main.rs` | After distribution loop (L5413) | Add auto-fund: debit 10K MOLT from treasury, credit to genesis account |
| 9b | `scripts/fund-deployer.py` | Entire file | Add deprecation notice (keep for reference) |

**Auto-fund code:**
```rust
// Auto-fund genesis/deployer with 10K MOLT from treasury (operational fund)
let ops_fund_molt: u64 = 10_000;
let ops_fund_shells = Account::molt_to_shells(ops_fund_molt);
if let Some(ref dist_wallets) = genesis_wallet.distribution_wallets {
    if let Some(treasury_dw) = dist_wallets.iter().find(|dw| dw.role == "validator_rewards") {
        let mut treasury_acct = state.get_account(&treasury_dw.pubkey).ok().flatten()
            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
        if treasury_acct.spendable >= ops_fund_shells {
            treasury_acct.deduct_spendable(ops_fund_shells).ok();
            state.put_account(&treasury_dw.pubkey, &treasury_acct).ok();
            
            let mut genesis_acct = state.get_account(&genesis_pubkey).ok().flatten()
                .unwrap_or_else(|| Account::new(0, genesis_pubkey));
            genesis_acct.add_spendable(ops_fund_shells).ok();
            state.put_account(&genesis_pubkey, &genesis_acct).ok();
            
            info!("🔧 Auto-funded genesis/deployer with {} MOLT from treasury", ops_fund_molt);
        }
    }
}
```

**New test:** `test_genesis_auto_fund_from_treasury` — genesis account has 10K MOLT after boot.

---

### Task 10: Wire Founding Moltys Vesting

**Goal:** The `founding_moltys` allocation (100M MOLT) has a vesting schedule per the whitepaper: 6-month cliff, then 18-month linear vest. Implement as a time-locked account.

**Implementation:**
- At genesis, set `founding_moltys` account with `locked = amount` and `spendable = 0`
- Store vesting parameters in state: `founding_moltys_cliff_slot`, `founding_moltys_vest_end_slot`
- Add a periodic unlock check: each block, check if cliff has passed and unlock proportionally

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 10a | `core/src/state.rs` | New functions | `set_founding_vesting_params()`, `get_founding_vesting_params()` |
| 10b | `validator/src/main.rs` | Genesis distribution loop | Set founding_moltys with locked=100M, spendable=0 |
| 10c | `validator/src/main.rs` | Block production | Add periodic vesting unlock for founding_moltys |
| 10d | `core/src/consensus.rs` | New constants | `FOUNDING_CLIFF_SLOTS`, `FOUNDING_VEST_DURATION_SLOTS` |

**Vesting schedule:**
- Cliff: 6 months = ~6 × 30 × 24 × 3600 / 5 = 3,110,400 slots (at 5s heartbeat effective rate)
- Note: Slots run at 400ms but blocks at 5s. Use wall-clock time stored in genesis.
- Better approach: store cliff/vest as Unix timestamps, check against block timestamp.

**New tests:**
- `test_founding_moltys_locked_at_genesis` — spendable = 0 initially
- `test_founding_moltys_cliff_not_reached` — no unlock before 6 months
- `test_founding_moltys_partial_vest` — proportional unlock after cliff
- `test_founding_moltys_fully_vested` — 100% unlocked after 24 months

---

### Task 11: Wire Ecosystem Partnerships Spending

**Goal:** The `ecosystem_partnerships` wallet (100M MOLT) needs a spending mechanism — multi-sig controlled disbursement for bridges, integrations, and partnerships.

**Implementation:**
- Spending requires a multi-sig transaction (uses existing MultiSigConfig)
- The wallet is identified via `ecosystem_partnerships_pubkey` in state (from Task 4)
- Add an RPC handler for `requestEcosystemGrant` that creates a multi-sig proposal

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 11a | `rpc/src/lib.rs` | New handler | `requestEcosystemGrant` — creates multi-sig proposal for ecosystem spending |
| 11b | `core/src/processor.rs` | Transfer validation | Allow transfers FROM ecosystem_partnerships if multi-sig approved |

**New test:** `test_ecosystem_grant_requires_multisig` — transfer from ecosystem wallet requires threshold signatures.

---

### Task 12: Wire Reserve Pool Access

**Goal:** The `reserve_pool` wallet (100M MOLT) is an emergency reserve. Access requires governance vote with high threshold (75%+ supermajority).

**Implementation:**
- Add a governance-gated transfer mechanism
- Store `reserve_pool_pubkey` in state (from Task 4)
- Require supermajority (75%) vote to access

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 12a | `contracts/moltdao/src/lib.rs` | New function | `emergency_reserve_transfer` — requires 75% vote |
| 12b | `validator/src/main.rs` | Contract execution context | Make reserve_pool_pubkey available |

**New test:** `test_reserve_pool_requires_supermajority` — transfer requires 75%+ vote.

---

### Task 13: Update All Documentation

**Goal:** Every doc file with old tokenomics numbers must be updated.

**Files to update:**

| # | File | What to update |
|---|------|---------------|
| 13a | `TOKENOMICS.md` | All numbers: distribution table, reward rates, fee split, emission math, depletion timeline |
| 13b | `docs/WHITEPAPER.md` | Genesis Distribution, fee structure, vesting timeline, validator earnings examples |
| 13c | `docs/VISION.md` | Validator earnings section, fee references |
| 13d | `docs/ARCHITECTURE.md` | Fee references if any |
| 13e | `docs/PRICE_BASED_REWARDS.md` | All reward constants, base rates |
| 13f | `docs/ECONOMIC_REFERENCE.md` | Full reference table update |
| 13g | `docs/CONTRIBUTORY_STAKE.md` | Vesting examples, earnings timeline |
| 13h | `docs/consensus/CONTRIBUTORY_STAKE.md` | Same (duplicate file) |
| 13i | `docs/foundation/WHITEPAPER.md` | Genesis distribution, vesting |
| 13j | `docs/foundation/VISION.md` | Earnings references |
| 13k | `docs/skills/VALIDATOR_SKILL.md` | Bootstrap/vesting references |
| 13l | `skills/validator/CONTRIBUTORY_STAKE_GUIDE.md` | Bootstrap examples, earnings timeline |
| 13m | `DEX_COMPLETION_MILESTONE.md` | DEX rewards, depletion references |
| 13n | `CONTRIBUTING.md` | If has tokenomics references |
| 13o | `PRODUCTION_AUDIT_*.md` | Update any hardcoded numbers |

---

### Task 14: Update All Frontend/Monitoring

**Goal:** Explorer, wallet, monitoring, DEX UI must show correct numbers.

**Files to update:**

| # | File | What to update |
|---|------|---------------|
| 14a | `explorer/js/address.js` | L611 SLOTS_PER_YEAR, reward display, vesting calculations |
| 14b | `explorer/js/block.js` | L53 block reward display |
| 14c | `wallet/js/wallet.js` | L1620 BOOTSTRAP_GRANT display, vesting progress |
| 14d | `monitoring/js/monitoring.js` | L339-343 distribution wallet display, new fee split display |
| 14e | `dex/index.html` | L1139-1141 DEX fee split governance options |
| 14f | `monitoring/index.html` | Fee split display labels |

---

### Task 15: Update RPC Tokenomics Endpoint

**Goal:** The `/getTokenomics` or similar RPC endpoint must return all new values.

**Files to modify:**

| # | File | Line(s) | Change |
|---|------|---------|--------|
| 15a | `rpc/src/lib.rs` | L9063-9077 | Update all returned values: rewards, decay, fee split, bootstrap |
| 15b | `rpc/src/lib.rs` | L4791-4831 | Update reward rate calculations with decay |
| 15c | `rpc/src/lib.rs` | L1941-1943 | Update block reward references |

---

### Task 16: Build, Reset, Test, Verify

**Goal:** Full clean build, reset blockchain, verify all changes work.

**Steps:**
1. `cargo build --release --bin moltchain-validator` — clean build, zero errors
2. `cargo test -p moltchain-core` — all core tests pass
3. `cargo test -p moltchain-core --test production_readiness` — production readiness tests pass
4. `cargo test -p moltchain-core --test caller_verification` — genesis distribution test passes
5. Reset blockchain: kill validators, `./reset-blockchain.sh`
6. Start 3 validators
7. Verify:
   - Block rewards are 0.1 / 0.05 MOLT (not 0.9 / 0.135)
   - Fee split is 40/30/10/10/10
   - All 6 wallet pubkeys stored and queryable
   - Community treasury receives 10% of fees
   - Producer fee share goes through vesting (50% debt repay if bootstrapping)
   - DEX reward pool is 100K/month
   - Genesis/deployer has 10K MOLT auto-funded
   - All validators at 100K MOLT, zero slashing
   - 5s block spacing maintained

---

## Commit Strategy

Each task gets its own commit:
1. `feat: reduce block rewards 0.1/0.05 MOLT — sustainable emission rate`
2. `feat: implement 20% annual reward decay — treasury never depletes`
3. `feat: update genesis split to 10/25/35/10/10/10 — all wallets get purpose`
4. `feat: store all 6 wallet pubkeys in state — enable wallet identification`
5. `feat: new fee split 40/30/10/10/10 — add community treasury + fee recycling`
6. `feat: route producer fees through vesting — fees repay bootstrap debt`
7. `feat: wire community treasury DAO spending — governance can spend`
8. `feat: wire builder grants as DEX reward source — 100K/month`
9. `feat: auto-fund genesis/deployer 10K from treasury — no manual script`
10. `feat: wire founding moltys 6-mo cliff + 18-mo vest — per whitepaper`
11. `feat: wire ecosystem partnerships multi-sig spending`
12. `feat: wire reserve pool governance-gated access`
13. `docs: update all documentation with new tokenomics`
14. `feat: update all frontends/monitoring with new numbers`
15. `feat: update RPC tokenomics endpoint`
16. `test: full integration verification — all tokenomics changes`

---

## Sustainability Projection (New Numbers)

**Year 1 (10 validators, 100K tx/day):**
- Block rewards: 17,280 blocks/day × 0.075 avg = 1,296 MOLT/day → 473K/year
- Fee income: 100K × 0.001 = 100 MOLT/day → 36.5K/year
- Fee recycling (10%): 10 MOLT/day → 3.65K/year
- Treasury net: -473K + 3.65K = **-469K** (100M pool barely touched)

**Year 5 (500 validators, 100M tx/day):**
- Block rewards (decayed 4×): 1,296 × 0.41 = 531 MOLT/day → 194K/year
- Fee income: 100M × 0.001 = 100K MOLT/day → 36.5M/year
- Fee recycling (10%): 10K MOLT/day → 3.65M/year
- Treasury net: -194K + 3.65M = **+3.46M** ← POSITIVE, treasury growing

**Crossover point: ~Year 3** — fee recycling exceeds block reward drain.

**Treasury at Year 10:** 100M - 2.7M (cumulative drain) + 15M (fee recycling) = **~112M** ← growing

---

## Risk Analysis

| Risk | Impact | Mitigation |
|------|--------|------------|
| Reward too low for early validators | Low validator participation | 100K MOLT bootstrap grant + 50% liquid from day 1 covers costs |
| Fee recycling insufficient | Treasury depletion | Decay ensures drain approaches 0; fee recycling only needs modest volume |
| Community treasury DAO exploited | Loss of 250M MOLT | Multi-sig + governance thresholds + time-lock on large proposals |
| Breaking change in genesis | Incompatible with existing state | Full blockchain reset required (acceptable for pre-mainnet) |
| Regression in vesting | Validators lose rewards | Comprehensive test suite for all vesting paths |

---

## Notes

- This plan requires a **full blockchain reset** since genesis distribution changes
- All validators will be re-bootstrapped with the new distribution
- The new system is designed for **zero inflation** — no new MOLT is ever minted
- All 6 wallets will have active spending mechanisms, no idle capital
- Fee recycling creates a self-sustaining loop at scale
- Reward decay ensures the treasury lasts 200+ years even without fee recycling
