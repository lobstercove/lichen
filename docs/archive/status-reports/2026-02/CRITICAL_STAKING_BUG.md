# 🚨 CRITICAL STAKING BUG - IMMEDIATE FIX REQUIRED

**Date:** February 8, 2026  
**Priority:** 🔴 **CRITICAL - BLOCKING LAUNCH**  
**Impact:** Total stake showing 1B MOLT instead of ~30-50K MOLT

---

## 🐛 BUG DESCRIPTION

### Symptom:
```
Total Stake: 1,000,030,002.40 MOLT
```

Should be: **~30,000-50,000 MOLT** (3-5 validators × 10K each)

### Root Cause:
Validator `6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H` has **1 BILLION MOLT** staked.  
This is the **genesis wallet pubkey** being counted as a validator with its full account balance!

### Data Analysis:
```
Validator #1 (2kRPL2NX): 0.0000 MOLT      ❌ Wrong (should have 10K)
Validator #2 (32rBqfmB): 10000.2970 MOLT  ✓  Correct
Validator #3 (52o6ZABr): 10001.2015 MOLT  ✓  Correct  
Validator #4 (6YkFWKH9): 1000000000 MOLT  ❌ CRITICAL BUG (genesis wallet!)
Validator #5 (HD2t815t): 10000.9045 MOLT  ✓  Correct
```

---

## 🔍 TECHNICAL DIAGNOSIS

### Problem 1: Genesis Wallet as Validator
The genesis wallet (1B MOLT treasury) was incorrectly added to the validator set with its full balance.

**File:** `validator/src/main.rs` lines ~309-315
**Issue:** Dynamic validator addition used account balance instead of proper staking

### Problem 2: No Separation of Balance Types
`Account.shells` field mixes:
- ✅ Spendable balance
- ❌ Staked balance (locked)
- ❌ Locked balance (in contracts, escrow, etc.)

**Required fields:**
```rust
pub struct Account {
    pub shells: u64,           // Total balance
    pub spendable: u64,        // Available for spending
    pub staked: u64,           // Locked in staking
    pub locked: u64,           // Locked in contracts/escrow
    // ...
}
```

### Problem 3: ValidatorInfo.stake Not Tracked Properly
`ValidatorInfo.stake` is set at validator creation but never updated when:
- Validator stakes more
- Validator unstakes
- Rewards are claimed
- Slashing occurs

**File:** `core/src/consensus.rs` ValidatorInfo struct

### Problem 4: Decimal Rounding Errors
```
32rBqfmB: 10000.2970 MOLT
52o6ZABr: 10001.2015 MOLT
HD2t815t: 10000.9045 MOLT
```

The `.2970`, `.2015`, `.9045` decimals don't make sense for staking.  
**Root cause:** Debt repayment or rewards are being added to stake counter instead of separate tracking.

---

## 🎯 REQUIRED FIXES

### Fix 1: Remove Genesis Wallet from Validators (IMMEDIATE)
**Effort:** 30 minutes  
**Priority:** 🔴 Critical

Delete validator entry for `6YkFWKH9` from database or reset validator set.

**Steps:**
1. Add CLI command `molt admin delete-validator <pubkey>`
2. Run `molt admin delete-validator 6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H`
3. Restart validator

### Fix 2: Separate Balance Types in Account (2-3 hours)
**Priority:** 🔴 Critical

**Update Account struct:**
```rust
pub struct Account {
    pub shells: u64,           // Total balance = spendable + staked + locked
    pub spendable: u64,        // Available to spend (shells - staked - locked)
    pub staked: u64,           // Locked in staking
    pub locked: u64,           // Locked in contracts/escrow
    pub data: Vec<u8>,
    pub owner: Pubkey,
    pub executable: bool,
}

impl Account {
    pub fn new(molt: u64, owner: Pubkey) -> Self {
        let shells = Self::molt_to_shells(molt);
        Account {
            shells,
            spendable: shells,  // Initially all spendable
            staked: 0,
            locked: 0,
            data: Vec::new(),
            owner,
            executable: false,
        }
    }
    
    pub fn stake(&mut self, amount: u64) -> Result<(), String> {
        if self.spendable < amount {
            return Err("Insufficient spendable balance".to_string());
        }
        self.spendable -= amount;
        self.staked += amount;
        Ok(())
    }
    
    pub fn unstake(&mut self, amount: u64) -> Result<(), String> {
        if self.staked < amount {
            return Err("Insufficient staked balance".to_string());
        }
        self.staked -= amount;
        self.spendable += amount;
        Ok(())
    }
}
```

### Fix 3: Update RPC Responses (1 hour)
**Priority:** 🔴 Critical

Update `/getBalance` endpoint:
```json
{
  "total_molt": 10000.0,
  "spendable_molt": 5000.0,
  "staked_molt": 5000.0,
  "locked_molt": 0.0,
  "shells": 10000000000000,
  "spendable_shells": 5000000000000,
  "staked_shells": 5000000000000,
  "locked_shells": 0
}
```

### Fix 4: Fix StakePool Integration (2-3 hours)
**Priority:** 🔴 Critical

**Files:** `core/src/consensus.rs` StakePool

Ensure:
1. `stake()` method updates both Account.staked AND ValidatorInfo.stake
2. `unstake()` decrements both
3. Rewards update ValidatorInfo.stake but not Account.staked (unless claimed)
4. Auto-claim on unstake

### Fix 5: Fix Metrics Calculation (30 minutes)
**Priority:** 🔴 Critical

**File:** `rpc/src/lib.rs` getMetrics endpoint

```rust
// Get total staked from StakePool, NOT from validator set
let stake_pool = state.state.get_stake_pool()?;
let total_staked = stake_pool.total_staked();  // Accurate count

// OR sum from accounts
let total_staked: u64 = validators.iter()
    .filter_map(|v| state.state.get_account(&v.pubkey).ok().flatten())
    .map(|acc| acc.staked)  // Use Account.staked, not ValidatorInfo.stake
    .sum();
```

### Fix 6: Prevent Genesis Wallet from Validating (15 minutes)
**Priority:** 🟡 Medium

**File:** `validator/src/main.rs` lines ~306-315

```rust
// Don't allow genesis wallet to become a validator
if validator_pubkey == genesis_pubkey {
    return Err("Genesis wallet cannot be a validator".to_string());
}
```

---

## 📊 VERIFICATION TESTS

After fixes, verify:

```bash
# 1. Check validator count
molt validators | grep "Total:"
# Should show: Total: 3-5 validators, 30000-50000 MOLT staked

# 2. Check metrics
molt metrics | grep "Staked:"
# Should show: Staked: 30000-50000 MOLT

# 3. Check individual validator
molt validator HD2t815ttu5XRosTyKwjn5Eq7jhmJeBGGVrYWDwSRSch
# Stake should be exactly 10000.0000 MOLT (no decimals)

# 4. Check balance breakdown
molt balance HD2t815ttu5XRosTyKwjn5Eq7jhmJeBGGVrYWDwSRSch
# Should show:
# Total: 10000 MOLT
# Spendable: 0 MOLT
# Staked: 10000 MOLT
```

---

## 🚀 IMPLEMENTATION PRIORITY

**Phase 1 - IMMEDIATE (1 hour):**
1. Add CLI command to delete bad validator
2. Delete genesis validator from database
3. Restart and verify metrics

**Phase 2 - CRITICAL (4-6 hours):**
1. Update Account struct with balance separation
2. Implement stake/unstake methods
3. Update all Account creation to use new fields
4. Update RPC responses

**Phase 3 - INTEGRATION (2-3 hours):**
1. Wire StakePool to Account updates
2. Test stake/unstake flow
3. Verify all metrics accurate
4. Add auto-claim on unstake

---

## 💥 IMPACT ASSESSMENT

**Before Fix:**
- ❌ Metrics showing 1B MOLT staked (999.97B error!)
- ❌ Cannot trust any staking data
- ❌ Genesis wallet could validate and earn rewards
- ❌ No way to see spendable vs staked balance
- ❌ Validators showing fractional MOLT (.2970, .2015, etc.)

**After Fix:**
- ✅ Accurate stake metrics (~30-50K MOLT)
- ✅ Clear separation of balance types
- ✅ Validators cannot stake genesis funds
- ✅ RPC shows spendable/staked/locked breakdown
- ✅ Clean whole numbers for staking (10000.0000)

---

## 🔴 BLOCKING STATUS

**This issue BLOCKS launch.** Cannot go to testnet/mainnet with:
- 1 billion MOLT miscounted as staked
- No balance type separation
- Validators with wrong stake amounts  
- Metrics showing completely wrong data

**Estimated fix time:** 8-10 hours of focused work

**Next immediate action:** Delete genesis validator entry from database.
