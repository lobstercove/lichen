# Validator Graduation & Anti-Fraud Plan

**Status:** Approved — Ready for Implementation  
**Date:** February 14, 2026  
**Affects:** `core/src/consensus.rs`, `validator/src/main.rs`, portal docs

---

## 1. Overview

MoltChain's Contributory Stake system gives the first 200 validators a 100,000 MOLT bootstrap grant from treasury so they can start validating with zero personal capital. This grant becomes a "debt" that validators repay through block rewards (50/50 split). Once repaid, they **graduate** and earn 100% liquid rewards.

**The problem:** At high validator counts, the fixed block reward pool is split thinner, making graduation take years (9.4 years at 500 heartbeat-only validators). We solve this with a time cap + performance bonus + hard cap on bootstrap grants.

---

## 2. Bootstrap Grant Rules

### Phase 1: Formation (Validators 1–200)

| Parameter | Value |
|-----------|-------|
| Bootstrap grant | 100,000 MOLT (from treasury) |
| Bootstrap debt | 100,000 MOLT |
| Reward split | 50% liquid / 50% debt repayment |
| Time cap | 547 days (18 months) |
| Performance bonus | 95%+ uptime → 1.5× repayment speed |
| Status at start | `Bootstrapping` |
| Status at graduation | `FullyVested` |

- All 200 formation validators get the **same grant** — no tiering, no class inequality.
- The 100K grant exceeds `MIN_VALIDATOR_STAKE` (75K) by 25K, giving validators a 25% buffer against slashing before deactivation.
- Grant is deducted from the 150M Validator Rewards pool in treasury.

### Phase 2: Mature (Validator 201+)

| Parameter | Value |
|-----------|-------|
| Bootstrap grant | **None** — bring your own 100K MOLT |
| Bootstrap debt | 0 |
| Status | `FullyVested` (immediate) |
| Reward split | 100% liquid from day 1 |

- Mature validators must fund their own 100K MOLT stake.
- They are immediately fully vested with no debt obligation.
- The network is proven by validator 201 — joining is an investment, not a leap of faith.

### Constants

```rust
/// Maximum number of validators eligible for bootstrap grants
pub const MAX_BOOTSTRAP_VALIDATORS: u64 = 200;

/// Maximum time (in slots) before remaining bootstrap debt is forgiven
/// 547 days × 216,000 slots/day = 118,152,000 slots
pub const MAX_BOOTSTRAP_SLOTS: u64 = 547 * 216_000;

/// Uptime threshold (basis points) for performance bonus
/// 9500 = 95.00%
pub const UPTIME_BONUS_THRESHOLD_BPS: u64 = 9500;

/// Performance bonus multiplier for debt repayment (basis points)
/// 15000 = 1.50× (the debt repayment portion gets multiplied by 1.5)
pub const PERFORMANCE_BONUS_BPS: u64 = 15000;
```

---

## 3. Graduation Mechanics

### 3.1 Normal Graduation (debt reaches 0)

Existing behavior — unchanged. Each `claim_rewards()`:
1. Total reward is split 50/50
2. Half goes to debt repayment (capped at remaining debt)
3. Half is liquid (spendable)
4. When `bootstrap_debt == 0` → status changes to `FullyVested`, `graduation_slot` is recorded

### 3.2 Performance Bonus (95%+ uptime → 1.5× repayment)

For validators with uptime ≥ 95%, the debt repayment portion is multiplied by 1.5×:

```
Standard:    reward=100 → 50 liquid, 50 to debt
With bonus:  reward=100 → 25 liquid, 75 to debt (1.5 × 50)
```

**Uptime calculation:**
```
uptime_bps = (blocks_produced × 10000) / expected_blocks_since_join
```

Where `expected_blocks_since_join` = `(current_slot - joined_slot) / num_validators` (approximate share of slots this validator should have been leader for).

**Implementation:** In `claim_rewards()`, check uptime before splitting. If uptime ≥ 95%, debt payment = `min(total_reward * 3/4, bootstrap_debt)`, liquid = `total_reward - debt_payment`.

### 3.3 Time-Cap Graduation (18 months)

After 547 days (~118M slots) of active block production, any remaining debt is **forgiven**:

```rust
// In claim_rewards() or a periodic check:
if current_slot - start_slot >= MAX_BOOTSTRAP_SLOTS && bootstrap_debt > 0 {
    bootstrap_debt = 0;
    status = FullyVested;
    graduation_slot = Some(current_slot);
}
```

**What "active" means:** The validator must have actually produced blocks. The check uses `start_slot` (when they first staked), not wall-clock time. If a validator goes offline for 6 months and comes back, those 6 months don't count toward the cap — they count from stake creation slot, but the validator must still be in the active set.

### 3.4 Graduation Timeline Estimates (200 validators, heartbeat-only)

| Scenario | Daily repayment/validator | Natural graduation | With time cap |
|----------|--------------------------|-------------------|---------------|
| Standard (50/50) | 73 MOLT/day | 1,370 days (3.7 yrs) | **547 days (cap)** |
| 95%+ uptime (75/25) | 109.5 MOLT/day | 913 days (2.5 yrs) | **547 days (cap)** |
| Mixed tx blocks (50%) | ~365 MOLT/day | 274 days | 274 days (natural) |
| High tx volume | ~730 MOLT/day | 137 days | 137 days (natural) |

At 50 validators the standard path graduates in ~343 days (no cap needed). The cap matters most when the network grows past ~70 validators under heartbeat-only conditions.

---

## 4. Anti-Fraud: Machine Fingerprint

### 4.1 The Attack

Without protection, an attacker can:
1. Run 50 validator processes on one machine, each with a different keypair
2. Each claims a 100K MOLT bootstrap grant = 5M MOLT ($500K) stolen from treasury
3. All 50 "validators" are actually one machine contributing nothing extra to decentralization

### 4.2 The Defense: Machine Fingerprint

Each validator collects a **machine fingerprint** — a SHA-256 hash of hardware identifiers unique to each physical/virtual machine:

**macOS:**
```
fingerprint = SHA-256(IOPlatformUUID + primary_MAC_address)
```
- `IOPlatformUUID`: from `ioreg -rd1 -c IOPlatformExpertDevice | grep IOPlatformUUID`
- Primary MAC: from `ifconfig en0 | grep ether`

**Linux:**
```
fingerprint = SHA-256(/etc/machine-id + primary_MAC_address)
```
- `/etc/machine-id`: unique per OS install, generated at first boot
- Primary MAC: from `/sys/class/net/<interface>/address`

### 4.3 Fingerprint Registration

The fingerprint is:
1. **Collected at validator startup** (before bootstrap grant)
2. **Signed with the validator's keypair** (proves the validator owns this fingerprint)
3. **Included in the validator announcement** message
4. **Stored in `StakePool`** as a `HashMap<[u8; 32], Pubkey>` — mapping fingerprint → validator pubkey

**Rule:** A bootstrap grant is ONLY issued if the fingerprint is not already registered to another active bootstrap validator. Self-funded validators (201+) skip this check entirely — they're spending their own money.

### 4.4 What This Prevents vs. What It Doesn't

| Scenario | Prevented? | Reason |
|----------|-----------|--------|
| 50 processes, 1 machine, 50 keys | **YES** | Same fingerprint for all 50 |
| 50 cheap VPS instances, 50 keys | **NO** | 50 real machines = 50 real fingerprints |
| Clone a VM with same machine-id | **YES** | Same machine-id → same fingerprint |
| Spoof MAC + machine-id | **Difficult** | Requires root AND knowledge of an unused pair |

**50 VPS instances is actually fine** — that's 50 real machines in potentially 50 different data centers contributing real decentralization. The grant is meant to lower the capital barrier, not prevent cloud deployment. Each VPS has real hosting costs ($10-20/month), so running 50 costs $500-1000/month — a natural economic deterrent.

### 4.5 Dev Mode

For local testing (running 3 validators on one machine):

```bash
# Skip fingerprint uniqueness check
moltchain-validator --dev-mode --p2p-port 8000
moltchain-validator --dev-mode --p2p-port 8001
moltchain-validator --dev-mode --p2p-port 8002
```

`--dev-mode` flag:
- Skips machine fingerprint uniqueness check
- Allows multiple validators per machine
- Sets fingerprint to `SHA-256(pubkey)` (unique per key, not per machine)
- **MUST NOT** be usable on mainnet — enforced by checking `chain_id`

```rust
// Dev mode only allowed on testnet
if dev_mode && chain_id.contains("mainnet") {
    panic!("--dev-mode is not allowed on mainnet");
}
```

---

## 5. Validator Machine Migration

### 5.1 The Problem

A validator runs for 100 days on Machine A, then wants to switch to Machine B (hardware upgrade, data center move, etc.). The validator's keypair file is portable — just copy it to the new machine. But the machine fingerprint changes.

### 5.2 The Solution: Automatic Fingerprint Update

When a validator announces with:
- A **known pubkey** (already in the validator set)
- A **new fingerprint** (different from the one on record)

The network treats this as a **machine migration**:

1. The old fingerprint is released from the registry
2. The new fingerprint is registered to this pubkey
3. Bootstrap debt, earned amount, graduation progress — all preserved (tied to pubkey, not machine)
4. The validator continues where it left off

**Using `--import-key`:**
1. Stop validator on old machine
2. On new machine, start with: `moltchain-validator --import-key /path/to/keypair.json`
3. The keypair file is copied into the validator's data directory
4. Fingerprint auto-updates on next announcement
5. All progress (debt, earned rewards, graduation) is preserved — tied to pubkey, not machine

### 5.2.1 Key Import vs Fresh Start

```bash
# FRESH START — new keypair generated, eligible for bootstrap grant if < 200
moltchain-validator --p2p-port 8000

# RESUME — import existing keypair, NO bootstrap grant (system detects existing stake)
moltchain-validator --p2p-port 8000 --import-key /path/to/validator-keypair.json
```

**Rule:** When `--import-key` is provided, the validator binary:
1. Reads the keypair file and copies it to `<data-dir>/validator-keypair.json`
2. Derives the pubkey from the imported key
3. On announcement, the network recognizes the existing pubkey → no new bootstrap grant
4. The validator's `StakeInfo` (debt, earned amount, blocks produced) is already in StakePool
5. Only the machine fingerprint is updated (migration flow)

**When NO key is provided:**
1. If `<data-dir>/validator-keypair.json` exists → use it (restart on same machine)
2. If it doesn't exist → generate a new keypair (fresh validator, eligible for grant)

### 5.3 Cooldown Period

To prevent rapid fingerprint cycling (which could be used to help a friend register on the "freed" machine):

```rust
/// Minimum slots between machine migrations (1 epoch = 432,000 slots ≈ 2 days)
pub const MIGRATION_COOLDOWN_SLOTS: u64 = 432_000;
```

If a validator tries to migrate again within 2 days: announcement is accepted (validator stays active), but the old fingerprint is **not released** for 2 days. This prevents:
- Validator A migrates off Machine X → Machine Y
- Validator B immediately claims Machine X's fingerprint for a bootstrap grant
- Validator A migrates back to Machine X within minutes

---

## 6. Same Wallet, Multiple Machines — NOT Allowed

### Why Not?

Running the same keypair on 3 machines simultaneously means all 3 try to produce blocks when that validator is selected as leader. This causes:

1. **DoubleBlock slashing** — Two different blocks for the same slot (severity: 100, maximum)
2. **DoubleVote slashing** — Conflicting votes from the same validator
3. **Network confusion** — Peers receive contradictory messages signed by the same key

From [consensus.rs](core/src/consensus.rs) `SlashingOffense`:

```rust
/// Validator produced two different blocks for the same slot
DoubleBlock {
    slot: u64,
    block_hash_1: Hash,
    block_hash_2: Hash,
}
// Severity: 100 (maximum — direct attack on consensus)
```

### The Rule

**One keypair = one machine = one validator.** This is not a MoltChain limitation — it's fundamental to Byzantine fault tolerance. Every PoS chain enforces this (Ethereum, Cosmos, Solana).

If you want to run 3 validators, you need:
- 3 different keypairs
- 3 different machines (or `--dev-mode` for testing)
- 3 separate bootstrap grants (if within the first 200) or 3 × 100K MOLT self-funded

---

## 7. Data Model Changes

### 7.1 `StakeInfo` (core/src/consensus.rs)

Add fields:

```rust
pub struct StakeInfo {
    // ... existing fields ...

    /// Machine fingerprint hash (SHA-256 of hardware identifiers)
    #[serde(default)]
    pub machine_fingerprint: [u8; 32],

    /// Slot when this validator first staked (for time-cap graduation)
    #[serde(default)]
    pub start_slot: u64,

    /// Bootstrap validator index (0-199 for formation, u64::MAX for self-funded)
    #[serde(default = "default_bootstrap_index")]
    pub bootstrap_index: u64,

    /// Last machine migration slot (for cooldown tracking)
    #[serde(default)]
    pub last_migration_slot: u64,
}

fn default_bootstrap_index() -> u64 { u64::MAX }
```

### 7.2 `StakePool` (core/src/consensus.rs)

Add fields:

```rust
pub struct StakePool {
    // ... existing fields ...

    /// Machine fingerprint registry: fingerprint → validator pubkey
    #[serde(default)]
    fingerprint_registry: HashMap<[u8; 32], Pubkey>,

    /// Counter of bootstrap grants issued (0–200)
    #[serde(default)]
    bootstrap_grants_issued: u64,
}
```

### 7.3 `ValidatorAnnouncement` (validator P2P message)

Add field to the announcement struct (in validator/src/main.rs):

```rust
pub machine_fingerprint: [u8; 32],
```

The signature covers: `pubkey + stake + current_slot + machine_fingerprint`

---

## 8. Code Changes Summary

### 8.1 `core/src/consensus.rs`

| Change | Description |
|--------|-------------|
| New constants | `MAX_BOOTSTRAP_VALIDATORS`, `MAX_BOOTSTRAP_SLOTS`, `UPTIME_BONUS_THRESHOLD_BPS`, `PERFORMANCE_BONUS_BPS`, `MIGRATION_COOLDOWN_SLOTS` |
| `StakeInfo` new fields | `machine_fingerprint`, `start_slot`, `bootstrap_index`, `last_migration_slot` |
| `StakeInfo::new()` | Accept `bootstrap_index` param; only create bootstrap debt if index < 200 |
| `StakeInfo::claim_rewards()` | Add time-cap check, add performance bonus multiplier |
| `StakeInfo::uptime_bps()` | New method — calculate uptime from blocks_produced vs expected |
| `StakePool` new fields | `fingerprint_registry`, `bootstrap_grants_issued` |
| `StakePool::stake()` | Check fingerprint uniqueness before bootstrap grant |
| `StakePool::register_fingerprint()` | New method — register/update fingerprint |
| `StakePool::migrate_fingerprint()` | New method — handle machine migration with cooldown |
| `StakePool::next_bootstrap_index()` | New method — return current count, increment if < 200 |

### 8.2 `validator/src/main.rs`

| Change | Description |
|--------|-------------|
| `collect_machine_fingerprint()` | New function — hash platform UUID + MAC |
| `--dev-mode` flag | Parse CLI arg, set dev fingerprint = SHA-256(pubkey) |
| Announcement struct | Add `machine_fingerprint` field |
| Announcement signature | Include fingerprint in signed message |
| Bootstrap grant logic | Check `bootstrap_grants_issued < 200` before granting |
| Fingerprint validation | Reject bootstrap if fingerprint already registered |
| Migration detection | Auto-update fingerprint when known pubkey announces from new machine |

### 8.3 Portal Updates

| File | Change |
|------|--------|
| `developers/validator.html` | Document bootstrap grant cap (200), time-cap graduation, machine migration |
| `website/index.html` | Update validator section to mention "first 200 validators" |
| `docs/ECONOMIC_REFERENCE.md` | Add graduation cap details, update scenario tables |

---

## 9. Test Plan

| Test | Description |
|------|-------------|
| `test_bootstrap_grant_first_200` | Validators 1–200 get bootstrap debt; validator 201 gets none |
| `test_bootstrap_counter_persists` | Counter survives restart/serialization |
| `test_time_cap_graduation` | Debt forgiven after MAX_BOOTSTRAP_SLOTS |
| `test_performance_bonus` | 95%+ uptime gets 1.5× repayment in claim_rewards |
| `test_performance_no_bonus` | < 95% uptime gets standard 50/50 split |
| `test_fingerprint_blocks_duplicate` | Same fingerprint, different pubkey → bootstrap denied |
| `test_fingerprint_allows_self_funded` | Duplicate fingerprint OK if validator 201+ (no bootstrap) |
| `test_machine_migration` | Known pubkey + new fingerprint → fingerprint updated, debt preserved |
| `test_migration_cooldown` | Rapid migration doesn't release old fingerprint immediately |
| `test_dev_mode_skips_fingerprint` | `--dev-mode` allows same-machine validators |
| `test_dev_mode_blocked_mainnet` | `--dev-mode` + mainnet chain_id → panic |
| `test_same_wallet_double_block` | Same key on 2 machines → DoubleBlock slashing (existing test) |
| `test_graduation_backward_compat` | Existing validators (pre-upgrade) graduate normally |

---

## 10. Backward Compatibility

Existing validators created before this upgrade:
- `bootstrap_index` defaults to `u64::MAX` via `#[serde(default)]` — treated as "unknown era"
- `machine_fingerprint` defaults to `[0; 32]` — zero fingerprint
- `start_slot` defaults to `0`
- These validators continue with the **existing** graduation logic (debt-based, no time cap)
- On next announcement, their fingerprint is populated from their machine
- They are NOT counted toward the 200 bootstrap cap (grandfathered)

For a fresh network (genesis), the counter starts at 0 and counts up as validators join.

---

## 11. Marketing Angle

> **First 200 validators: $0 to start.**  
> We stake you 100,000 MOLT to build MoltChain with us.  
> After 200? Bring your own stake — the network is proven.

- **Clean story**: No tiering, no class system. 200 is exclusive enough to be special, large enough for real decentralization.
- **Performance matters**: Hit 95% uptime and graduate faster.
- **18-month guarantee**: Worst case, you're free in 18 months. Best case, weeks.
- **Machine portable**: Switch hardware anytime — your progress follows your key.

---

*Last updated: February 14, 2026*
