# DEX Completion Milestone — Full Plan

**Date:** February 13, 2026  
**Status:** Planning  
**Branch:** `main`  
**Commit baseline:** `8710e12`

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Token Economics & Trading Flow](#2-token-economics--trading-flow)
3. [Phase 0 — Tokenomics & Distribution Alignment](#3-phase-0--tokenomics--distribution-alignment)
4. [Phase 1 — Critical Fixes (Foundation)](#4-phase-1--critical-fixes-foundation)
5. [Phase 2 — DEX Contract Enhancements](#5-phase-2--dex-contract-enhancements)
6. [Phase 3 — Security & Cross-Contract Wiring](#6-phase-3--security--cross-contract-wiring)
7. [Phase 4 — Build, Test, Deploy](#7-phase-4--build-test-deploy)
8. [Contract Audit Summary](#8-contract-audit-summary)
9. [Genesis Initialization Order](#9-genesis-initialization-order)
10. [Leverage Tier Table](#10-leverage-tier-table)
11. [Candle Interval Table](#11-candle-interval-table)
12. [Open Questions & Future Work](#12-open-questions--future-work)

---

## 1. Executive Summary

This milestone brings the DEX from "deployed bytecode" to "fully operational from block 0." The scope covers:

- **Tokenomics alignment** → fix genesis distribution mismatch between whitepaper/multisig.rs and genesis.rs/website, readjust all MOLT-denominated parameters for launch price
- **3 broken wrapped token WASMs** → fix exports, recompile
- **Genesis initialization** → contracts are deployed but never initialized; add a post-deploy phase that calls `initialize()` on all 26 contracts
- **MOLT auto-listing** → create MOLT/mUSD trading pair at genesis
- **DEX enhancements** → 100x leverage, 9 candle intervals, real cross-contract calls, collateral locking, insurance fund governance
- **Security fixes** → oracle hash, reputation verification, token transfer wiring
- **Full test suite** → 333+ tests pass, clippy clean, format clean

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| DEX quote currency | **mUSD** for all trading pairs | Stablecoin-denominated trading, consistent pricing |
| Launchpad currency | **MOLT** for token purchases | Drives MOLT demand, increases utility and volume |
| User onramp | Buy MOLT with USDT/USDC/SOL/ETH via bridge + wrapped tokens | Already planned in core code and bridge contract |
| Graduation currency | **MOLT** (1M MOLT market cap = $100K) | Consistent with launchpad denomination |
| Insurance fund governance | DAO-controlled withdrawal | Prevents single-point admin abuse |

---

## 2. Token Economics & Trading Flow

### User Journey

```
External Chain                    MoltChain
─────────────                    ─────────
USDT/USDC  ──bridge──►  mUSD (wrapped stablecoin, 6 decimals)
SOL        ──bridge──►  WSOL (wrapped SOL, 9 decimals)
ETH        ──bridge──►  WETH (wrapped ETH, 9 decimals)

mUSD ──DEX swap──► MOLT     (primary pair: MOLT/mUSD)
WSOL ──DEX swap──► MOLT     (via WSOL/mUSD → mUSD/MOLT route)
WETH ──DEX swap──► MOLT     (via WETH/mUSD → mUSD/MOLT route)
```

### Trading Pairs (All quoted in mUSD)

| Pair | Template | Created At |
|------|----------|-----------|
| MOLT/mUSD | Primary | Genesis (auto-listed) |
| WSOL/mUSD | Wrapped | Genesis (auto-listed) |
| WETH/mUSD | Wrapped | Genesis (auto-listed) |
| TOKEN/mUSD | User-deployed | Via governance or ClawPump graduation |

### Launchpad Flow (ClawPump — denominated in MOLT)

```
1. Creator calls create_token() → pays 10 MOLT creation fee
2. Users buy tokens with MOLT on bonding curve
3. Market cap reaches 1,000,000 MOLT → token graduates
4. On graduation:
   a. Accumulated MOLT from bonding curve is used as liquidity
   b. Token pair TOKEN/mUSD created on dex_core
   c. Pool TOKEN/mUSD created on dex_amm
   d. Initial liquidity seeded (MOLT converted to mUSD equivalent)
5. After graduation: buy/sell on bonding curve blocked → trade on DEX only
```

### Fee Burn Mechanism

- 40% of all transaction fees are burned (MOLT is deflationary)
- MOLT is NOT mintable (1B fixed supply since genesis)
- MOLT IS burnable (fee burn reduces circulating supply over time)

---

## 3. Phase 0 — Tokenomics & Distribution Alignment

### Task 0.1 — Fix Genesis Distribution Mismatch

**Problem:** `genesis.rs` and `website/index.html` have validator rewards = 250M (25%) and dev fund = 150M (15%). The whitepaper and `multisig.rs` have the correct values: validator rewards = 150M (15%), builder grants = 250M (25%). The values are swapped.

**Files to modify:**
- `core/src/genesis.rs` — swap validator_rewards and builder_grants allocations
- `website/index.html` — update pie chart / distribution section
- `WHITEPAPER.md` — fix stale deploy fee (says 0.0001 MOLT, should be 25 MOLT at $0.10)

**Correct genesis distribution (1B MOLT total):**

| Allocation | MOLT | % | Purpose |
|-----------|------|---|--------|
| Community Treasury | 250,000,000 | 25% | Ecosystem growth, governed by DAO |
| Builder Grants | 350,000,000 | 35% | DEX rewards, launchpad incentives |
| Validator Rewards | 100,000,000 | 10% | Block production rewards pool |
| Founding Team | 100,000,000 | 10% | 6-mo cliff + 18-mo vest |
| Ecosystem Partnerships | 100,000,000 | 10% | Strategic partners |
| Reserve Pool | 100,000,000 | 10% | Emergency buffer |

### Task 0.2 — Rename ANNUAL_INFLATION_BPS

**Problem:** `ANNUAL_INFLATION_BPS = 500` in `consensus.rs` implies MOLT is inflationary. It is NOT — MOLT is fixed supply, not mintable. Block rewards are drawn from the 100M validator rewards pool.

**Fix:** Rename to `ANNUAL_REWARD_RATE_BPS` across all files. Add comment: "Target annual draw rate from reward pool (informational — not used in calculation)."

### Task 0.3 — Launch Price Readjustment ($0.10/MOLT — LOCKED)

> **Price: $0.10 per MOLT — LOCKED IN**
> **FDV: $100,000,000** (1B × $0.10)
> **Launch: tradeable from block 0 on DEX, no private round, fair launch**

#### How the original values were set

**The entire system was designed at $1.00/MOLT.** Evidence:
- `reference_price_usd = 1.0` in `RewardConfig` (consensus.rs line ~54)
- At $1: deploy = $2.50, base fee = $0.00001 — matching the website/docs exactly
- Block rewards: 0.1 MOLT = $0.01/block at $0.10, designed to emit ~1.42% of supply/year at 100% activity
- Heartbeat: 0.05 MOLT = 50% of tx reward — design comment says "50% of transaction reward"
- The dynamic price adjustment (PriceOracle, get_adjusted_reward) exists in code but is **dormant** — MockOracle always returns $1.00, never called by the validator

**At $0.10, every value is 10× too cheap in USD.** We must multiply MOLT amounts by 10 to hit the same USD targets.

#### Category A — Core Chain Fees (MUST 10×)

These target specific USD prices documented on the website and whitepaper.

| # | Parameter | File:Line | Current Shells | Current MOLT | USD at $1 | USD at $0.10 | **New MOLT** | **New Shells** | USD Result |
|---|-----------|-----------|---------------|-------------|-----------|-------------|------------|--------------|------------|
| 1 | `BASE_FEE` | processor.rs:59 | 10,000 | 0.00001 | $0.00001 | $0.000001 | **0.001** | **1,000,000** | **$0.0001** |
| 2 | `CONTRACT_DEPLOY_FEE` | processor.rs:62 | 2,500,000,000 | 2.5 | $2.50 | $0.25 | **25** | **25,000,000,000** | **$2.50** |
| 3 | `CONTRACT_UPGRADE_FEE` | processor.rs:65 | 1,000,000,000 | 1.0 | $1.00 | $0.10 | **10** | **10,000,000,000** | **$1.00** |
| 4 | `NFT_MINT_FEE` | processor.rs:68 | 1,000,000 | 0.001 | $0.001 | $0.0001 | **0.1** | **100,000,000** | **$0.01** |
| 5 | `NFT_COLLECTION_FEE` | processor.rs:71 | 100,000,000,000 | 100 | $100 | $10 | **1,000** | **1,000,000,000,000** | **$100** |
| 6 | `base_fee_shells` | genesis.rs:~120 | 10,000 | — | — | — | — | **1,000,000** | (match #1) |
| 7 | `rent_rate_shells_per_kb_month` | genesis.rs:~125 | 1,000 | 0.000001 | $0.000001 | $0.0000001 | **0.00001** | **10,000** | $0.000001 |

#### Category B — Block Rewards (DECISION: 5× recommended)

Block rewards come from the **validator rewards pool (100M MOLT)**, not minted. At $1 the original design intent was:
- 0.18 MOLT/block = $0.18 per transaction block
- 0.027 MOLT/block = $0.027 per heartbeat (15% of tx reward)

> **✅ Applied:** 5× upgrade now live — 0.1 MOLT/tx, 0.05 MOLT/heartbeat

**At $0.10, keeping the same MOLT amounts gives validators only $0.018/block — 10× less than designed.**

Pool depletion analysis at various multipliers (assuming 3 validators, 50% tx activity):

| Variant | TX Reward | Heartbeat | USD/block | Annual Draw (MOLT) | Pool Life (100M) |
|---------|-----------|-----------|-----------|-------------------|------------------|
| 1× (no change) | 0.18 MOLT | 0.027 MOLT | $0.018 | ~8.1M | **~18.4 years** |
| 2× | 0.36 MOLT | 0.054 MOLT | $0.036 | ~16.3M | **~9.2 years** |
| **5× (recommended)** | **0.1 MOLT** | **0.05 MOLT** | **$0.01** | **~5.9M** | **~16.9 years** |
| 10× (full USD match) | 1.8 MOLT | 0.27 MOLT | $0.18 | ~81.3M | **~1.8 years** |

**Recommendation: 5×.** Rationale:
1. Validators earn $0.01/block — attractive enough for early adoption
2. Pool lasts ~2.5 years at 50% activity — enough time for fee revenue to grow
3. As DEX volume grows, validators earn more from the 30% fee share (sustainable long-term)
4. Post-depletion, validators live on fees alone (like Bitcoin post-halving)
5. The dormant dynamic adjustment mechanism (RewardConfig) can be activated later for fine-tuning

| # | Parameter | File:Line | Current Shells | Current MOLT | **New MOLT** | **New Shells** | USD at $0.10 |
|---|-----------|-----------|---------------|-------------|------------|--------------|-------------|
| 8 | `TRANSACTION_BLOCK_REWARD` | consensus.rs:16 | 180,000,000 | 0.18 | **0.1** | **100,000,000** | **$0.01** |
| 9 | `HEARTBEAT_BLOCK_REWARD` | consensus.rs:19 | 27,000,000 | 0.027 | **0.05** | **50,000,000** | **$0.005** |
| 10 | `BLOCK_REWARD` (legacy alias) | consensus.rs:22 | 180,000,000 | 0.18 | **0.1** | **100,000,000** | (match #8) |

#### Category C — Staking Thresholds (10×)

Original design: $10K min / $100K max to run a validator. At $0.10 those targets require 10× the MOLT.

| # | Parameter | File:Line | Current MOLT | USD at $0.10 | **New MOLT** | USD at $0.10 |
|---|-----------|-----------|-------------|-------------|------------|-------------|
| 11 | `MIN_VALIDATOR_STAKE` | consensus.rs:13 | 10,000 | $1,000 | **75,000** | **$7,500** |
| 12 | `MAX_VALIDATOR_STAKE` | consensus.rs:~148 | 100,000 | $10,000 | **1,000,000** | **$100,000** |

#### Category D — DEX Contract Parameters

| # | Parameter | File | Current | USD at $0.10 | **New Value** | USD at $0.10 | Rationale |
|---|-----------|------|---------|-------------|-------------|-------------|----------|
| 13 | `MAX_ORDER_SIZE` | dex_core:48 | 1,000 MOLT | $100 | **10,000,000 MOLT** | **$1,000,000** | Real DEX needs $1M max orders |
| 14 | `CREATION_FEE` | clawpump:42 | 0.1 MOLT | $0.01 | **10 MOLT** | **$1.00** | Anti-spam token creation |
| 15 | `GRADUATION_MARKET_CAP` | clawpump:48 | 100,000 MOLT | $10,000 | **1,000,000 MOLT** | **$100,000** | Original $100K target |
| 16 | `DEFAULT_MAX_BUY_AMOUNT` | clawpump:72 | 10,000 MOLT | $1,000 | **100,000 MOLT** | **$10,000** | Anti-whale cap |
| 17 | `MIN_LISTING_LIQUIDITY` | dex_governance:31 | 10 MOLT | $1 | **100,000 MOLT** | **$10,000** | Fix 1000× bug + scale |
| 18 | `REWARD_POOL_PER_MONTH` | dex_rewards:26 | 1,000,000 MOLT | $100K/mo | **100,000 MOLT** | **$10K/mo** | Draws from 350M builder grants, lasts 290+ years |
| 19 | `TIER_BRONZE_MAX` | dex_rewards:30 | 10,000 MOLT vol | $1K vol | **100,000 MOLT** | **$10K vol** | Scale volume tiers |
| 20 | `TIER_SILVER_MAX` | dex_rewards:31 | 100,000 MOLT vol | $10K vol | **1,000,000 MOLT** | **$100K vol** | Scale volume tiers |
| 21 | `TIER_GOLD_MAX` | dex_rewards:32 | 1,000,000 MOLT vol | $100K vol | **10,000,000 MOLT** | **$1M vol** | Scale volume tiers |

#### Category E — DAO & Governance (10×)

| # | Parameter | File | Current | USD at $0.10 | **New Value** | USD at $0.10 | Rationale |
|---|-----------|------|---------|-------------|-------------|-------------|----------|
| 22 | `PROPOSAL_STAKE` | moltdao:43 | 1,000 MOLT | $100 | **10,000 MOLT** | **$1,000** | Serious governance barrier |

#### Category F — Infrastructure (10×)

| # | Parameter | File | Current | **New Value** | Rationale |
|---|-----------|------|---------|-------------|----------|
| 23 | `MIN_STAKE_PER_GB` | reef_storage:42 | 0.001 MOLT | **0.01 MOLT** | 10× for storage provider stake |
| 24 | `REWARD_PER_SLOT_PER_BYTE` | reef_storage:37 | 1 shell | **10 shells** | 10× for storage rewards |

#### Category G — Reference Price & Config

| # | Parameter | File:Line | Current | **New Value** | Rationale |
|---|-----------|-----------|---------|-------------|----------|
| 25 | `reference_price_usd` | consensus.rs:~54 | 1.0 | **0.10** | Must match launch price for dormant oracle |
| 26 | `ANNUAL_INFLATION_BPS` | consensus.rs:25 | 500 | **Rename → `ANNUAL_REWARD_RATE_BPS`** | Not inflation — pool draw (Task 0.2) |
| 27 | `REWARD_POOL_MOLT` | main.rs:43 | 150,000,000 | **100,000,000** | Updated — genesis allocation |

#### Category H — Faucet (testnet only)

| # | Parameter | File | Current | **New Value** | Rationale |
|---|-----------|------|---------|-------------|----------|
| 28 | `max_per_request` | faucet main.rs | 10 MOLT | **100 MOLT** | 10× so testers get $10 worth |
| 29 | `daily_limit_per_ip` | faucet main.rs | 10 MOLT | **100 MOLT** | Match above |
| 30 | `MOLT_PER_REQUEST` | faucet.js | 10 | **100** | Match above |

#### Category N — No Change Needed (BPS/percentage-based, price-independent)

| Parameter | File | Value | Why No Change |
|-----------|------|-------|---------------|
| DEX maker fee | dex_core | -1 bps | Percentage of trade |
| DEX taker fee | dex_core | 5 bps | Percentage of trade |
| Fee protocol/LP/staker shares | dex_core | 60/20/20% | Ratios |
| AMM fee tiers | dex_amm | 1/5/30/100 bps | Percentage of trade |
| All margin params | dex_margin | bps | Percentage-based |
| Lending LTV/rates | lobsterlend | % and bps | Interest rates |
| Fee burn/producer/voter/treasury/community | processor/genesis | 40/30/10/10/10% | Ratios |
| All slashing percentages | consensus | % | Percentage-based |
| Reputation scores | moltyid | points | Not MOLT |
| ClawPump platform fee | clawpump | 1% | Percentage-based |
| Vault deposit/withdrawal fees | clawvault | 10/30 bps | Percentage-based |
| Referral rates | dex_rewards | bps | Percentage-based |
| Voting thresholds | dex_governance, moltdao | % | Percentage-based |
| Bonding curve slope | clawpump | ratio | Price discovery |
| Flash loan fee | lobsterlend, moltswap | 9 bps | Percentage-based |
| Marketplace fee | moltmarket, moltauction | 250 bps | Percentage-based |
| All timeouts/cooldowns | various | slots | Time-based |
| MIN_ORDER_VALUE | dex_core | 1,000 shells | Already negligible |

#### Reward Pool Sustainability Summary

| Source | Pool | Monthly Draw | Depletes In |
|--------|------|-------------|-------------|
| Block rewards (5× at $0.10) | Validator Rewards (100M) | ~3.4M MOLT | **~2.4 years** (50% activity) |
| DEX trading rewards | Builder Grants (350M) | 100K MOLT | **~290 years** |

**After validator reward pool depletion:** Validators earn from transaction fees only (30% block producer + 10% voter share + 10% community share). At scale, fee revenue exceeds pool rewards. The community treasury (250M) can vote to replenish via governance if needed.

#### Complete Readjustment Summary

**30 parameters total:**
- **7 core chain fees** in `processor.rs` and `genesis.rs`
- **3 block reward values** in `consensus.rs`
- **2 staking thresholds** in `consensus.rs`
- **9 DEX contract values** in `dex_core`, `dex_rewards`, `dex_governance`
- **3 ClawPump values** in `clawpump`
- **1 DAO value** in `moltdao`
- **2 infrastructure values** in `reef_storage`
- **3 config values** (reference_price, rename, faucet)
- **~20+ values confirmed NO CHANGE** (BPS/percentage-based, price-independent)

---

## 4. Phase 1 — Critical Fixes (Foundation)

### Task 1.1 — Fix Wrapped Token WASM Exports

**Problem:** wsol_token (86 B), weth_token (86 B), and musd_token (86 B) have real source code (585–881 lines each with full logic) but compile to empty 86-byte WASM because functions use `pub fn` instead of `#[no_mangle] pub extern "C" fn`.

**Files to modify:**
- `contracts/wsol_token/src/lib.rs` (585 lines)
- `contracts/weth_token/src/lib.rs` (585 lines)
- `contracts/musd_token/src/lib.rs` (881 lines)

**Fix:** Add `#[no_mangle] pub extern "C" fn` annotation to every public function. The reference implementation is `moltcoin/src/lib.rs` which uses this pattern correctly and compiles to 5,392 bytes.

**Functions to export (per contract):**

wsol_token / weth_token (17 functions each):
- `initialize`, `mint`, `burn`, `transfer`, `approve`, `transfer_from`
- `attest_reserves`, `balance_of`, `allowance`, `total_supply`
- `total_minted`, `total_burned`, `emergency_pause`, `emergency_unpause`
- `transfer_admin`, `get_reserve_ratio`, `get_epoch_remaining`

musd_token (17+ functions):
- Same as above plus proof-of-reserves attestation count tracking

**Validation:** After recompile, each WASM must be >1 KB (expected ~5–8 KB based on moltcoin reference).

### Task 1.2 — Genesis Contract Initialization

**Problem:** `genesis_auto_deploy()` in `validator/src/main.rs` stores WASM bytecode but never calls `initialize()`. All contract storage is empty — admin keys unset, counters at 0, config values missing. Contracts are deployed but non-functional.

**File to modify:** `validator/src/main.rs` (function `genesis_auto_deploy`, ~line 1411)

**Implementation:** Add a second phase after all contracts are deployed:

```
Phase 1 (existing): Deploy all 26 contracts (store bytecode, index, register symbols)
Phase 2 (new):      Initialize all contracts by executing their initialize() function
```

**Initialization calls (in dependency order):**

1. **Tokens first** (no dependencies):
   - `moltcoin.initialize(deployer_pubkey)` — sets admin, creates initial supply
   - `musd_token.initialize(deployer_pubkey)` — sets admin, zero initial supply
   - `wsol_token.initialize(deployer_pubkey)` — sets admin, zero initial supply
   - `weth_token.initialize(deployer_pubkey)` — sets admin, zero initial supply

2. **Identity** (used by many contracts):
   - `moltyid.initialize(deployer_pubkey)` — sets admin for identity system

3. **DEX core** (depends on tokens):
   - `dex_core.initialize(deployer_pubkey)` — sets admin, zero pair count
   - `dex_amm.initialize(deployer_pubkey)` — sets admin, zero pool count
   - `moltswap.initialize(deployer_pubkey)` — sets admin for legacy AMM
   - `dex_router.initialize(deployer_pubkey)` — sets admin + addresses of dex_core, dex_amm, moltswap
   - `dex_margin.initialize(deployer_pubkey)` — sets admin, insurance fund = 0
   - `dex_rewards.initialize(deployer_pubkey)` — sets admin, reward rates
   - `dex_governance.initialize(deployer_pubkey)` — sets admin, listing requirements
   - `dex_analytics.initialize(deployer_pubkey)` — sets admin

4. **DeFi protocols** (may reference DEX/tokens):
   - `lobsterlend.initialize(deployer_pubkey)` — lending protocol
   - `moltbridge.initialize(deployer_pubkey)` — cross-chain bridge
   - `moltoracle.initialize(deployer_pubkey)` — oracle feeds
   - `moltswap.initialize(deployer_pubkey)` — legacy AMM (if not already done)

5. **Infrastructure** (may reference identity):
   - `moltdao.initialize(deployer_pubkey)` — DAO governance
   - `moltmarket.initialize(deployer_pubkey)` — NFT marketplace
   - `moltpunks.initialize(deployer_pubkey)` — NFT collection
   - `moltauction.initialize(deployer_pubkey)` — auction house
   - `clawpump.initialize(deployer_pubkey)` — token launchpad
   - `clawvault.initialize(deployer_pubkey)` — yield aggregator
   - `clawpay.initialize(deployer_pubkey)` — streaming payments
   - `bountyboard.initialize(deployer_pubkey)` — bounty system
   - `compute_market.initialize(deployer_pubkey)` — compute marketplace
   - `reef_storage.initialize(deployer_pubkey)` — decentralized storage

**Technical approach:** The validator's WASM runtime is already available during genesis. After `put_account()` for each contract, we need to invoke the contract's `initialize` or `call` entry point with the appropriate arguments. This requires:
- Instantiating the WASM module via Wasmer
- Providing host functions (storage_read/write, log, etc.)
- Calling the exported function with serialized args
- Committing the resulting storage changes to the account

### Task 1.3 — MOLT Auto-Listing at Genesis

**File to modify:** `validator/src/main.rs` (after initialization phase)

**After all contracts are initialized, execute:**

1. `dex_core.create_pair("MOLT", "MUSD")` — creates the primary CLOB trading pair
2. `dex_core.create_pair("WSOL", "MUSD")` — creates WSOL/mUSD pair
3. `dex_core.create_pair("WETH", "MUSD")` — creates WETH/mUSD pair
4. `dex_amm.create_pool("MOLT", "MUSD", 30)` — creates 30bps fee tier AMM pool
5. `dex_amm.create_pool("WSOL", "MUSD", 30)` — creates WSOL/mUSD AMM pool
6. `dex_amm.create_pool("WETH", "MUSD", 30)` — creates WETH/mUSD AMM pool

**Note:** Initial liquidity seeding from treasury is a future step — these pairs exist but are empty until someone adds liquidity.

### Task 1.4 — mUSD as DEX Quote Currency

**Files to modify:**
- `contracts/dex_core/src/lib.rs` — ensure `create_pair` validates quote token or add a preferred quote constant
- `contracts/dex_governance/src/lib.rs` — listing proposals should default to mUSD quote
- `contracts/dex_analytics/src/lib.rs` — 24h stats denominated in mUSD

**ClawPump stays in MOLT** (no change needed — already uses MOLT).

---

## 5. Phase 2 — DEX Contract Enhancements

### Task 2.1 — dex_analytics: Add 3d, Weekly, Yearly Candle Intervals

**File:** `contracts/dex_analytics/src/lib.rs` (621 lines)

**Current intervals:** 1m, 5m, 15m, 1h, 4h, 1d (6 intervals)
**New intervals:** 3d, 1w, 1y (3 additional = 9 total)

**Changes:**
- Add constants: `INTERVAL_3D = 259_200`, `INTERVAL_1W = 604_800`, `INTERVAL_1Y = 31_536_000`
- Add to interval array and `get_retention()` function
- Update `record_trade()` to iterate over all 9 intervals
- Update `get_ohlcv()` to accept new interval values
- Storage keys: `ana_c_{pair}_3d_{idx}`, `ana_c_{pair}_1w_{idx}`, `ana_c_{pair}_1y_{idx}`

**Retention policies:**

| Interval | Period (s) | Max Candles | Retention |
|----------|-----------|-------------|-----------|
| 1m | 60 | 1,440 | 24 hours |
| 5m | 300 | 2,016 | 7 days |
| 15m | 900 | 2,880 | 30 days |
| 1h | 3,600 | 2,160 | 90 days |
| 4h | 14,400 | 2,190 | 365 days |
| 1d | 86,400 | 1,095 | 3 years |
| 3d | 259,200 | 243 | 2 years |
| 1w | 604,800 | 260 | 5 years |
| 1y | 31,536,000 | unlimited | forever |

### Task 2.2 — dex_margin: 100x Leverage with Tiered Parameters

**File:** `contracts/dex_margin/src/lib.rs` (826 lines)

**Current:** Fixed 5x max leverage, 20% initial margin, 10% maintenance margin, 5% liquidation penalty.

**New:** Up to 100x leverage with parameters that scale by tier.

**Changes:**
- Change `MAX_LEVERAGE` from `5` to `100`
- Replace flat margin/fee constants with a tier lookup function
- `open_position()` validates leverage is one of the allowed tiers OR any value up to 100x
- Maintenance margin, initial margin, and liquidation penalty scale with leverage
- Funding rate multiplier increases with leverage

**Leverage tier table (see Section 9 for full details):**

| Leverage | Initial Margin | Maintenance Margin | Liquidation Penalty | Funding Rate Mult |
|----------|---------------|-------------------|--------------------|--------------------|
| ≤2x | 50% | 25% | 3% | 1.0x |
| ≤3x | 33% | 17% | 3% | 1.0x |
| ≤5x | 20% | 10% | 5% | 1.5x |
| ≤10x | 10% | 5% | 5% | 2.0x |
| ≤25x | 4% | 2% | 7% | 3.0x |
| ≤50x | 2% | 1% | 10% | 5.0x |
| ≤100x | 1% | 0.5% | 15% | 10.0x |

**Implementation:** A function `get_tier_params(leverage: u64) -> (initial_margin_bps, maint_margin_bps, liq_penalty_bps, funding_mult)` returns the correct values based on which tier the requested leverage falls into.

### Task 2.3 — dex_margin: Host-Level Collateral Locking

**Files:**
- `contracts/dex_margin/src/lib.rs`
- `core/src/contract.rs` (if host function wiring needed)

**Problem:** Opening a margin position currently only updates the contract's internal storage — the trader's account-level `spendable` balance is not reduced. A trader could open multiple positions worth more than their balance.

**Fix:**
- On `open_position()`: call host function `lock(trader, margin_amount)` which moves funds from `spendable` → `locked` on the Account struct
- On `close_position()`: call host function `unlock(trader, margin_amount ± PnL)` to return funds
- On `liquidate()`: call host `unlock()` for remaining margin minus penalty, `deduct()` for penalty split
- The Account struct already supports `lock()/unlock()` with invariant enforcement (`shells == spendable + staked + locked`)

### Task 2.4 — dex_margin: Insurance Fund Withdrawal

**File:** `contracts/dex_margin/src/lib.rs`

**Problem:** Insurance fund accumulates via liquidation penalties but has no withdrawal mechanism.

**Add function:**
```
withdraw_insurance(amount: u64, recipient: Pubkey) -> Result
```
- Callable only by admin (initially deployer, transferable to DAO)
- Validates amount ≤ current insurance fund balance
- Transfers MOLT from insurance fund to recipient via cross-contract call
- Emits event for transparency
- Future: governance proposal required (via dex_governance or moltdao)

### Task 2.5 — dex_router: Real Cross-Contract Swap Execution

**File:** `contracts/dex_router/src/lib.rs` (861 lines)

**Problem:** `execute_clob_swap()`, `execute_amm_swap()`, `execute_legacy_swap()` are simulated — they deduct fees but don't make actual cross-contract calls.

**Fix:** Replace simulation with real cross-contract calls:
- `execute_clob_swap()` → calls `dex_core.place_order()` (market order)
- `execute_amm_swap()` → calls `dex_amm.swap_exact_in()`
- `execute_legacy_swap()` → calls `moltswap.swap_a_for_b()` or `swap_b_for_a()`

**Each call must:**
1. Build the correct argument buffer for the target contract
2. Use the host `cross_contract_call()` function
3. Parse the return value to verify success
4. Handle failures gracefully (revert route, return error)

### Task 2.6 — dex_rewards: Actual MOLT Token Transfer on Claim

**File:** `contracts/dex_rewards/src/lib.rs` (609 lines)

**Problem:** `claim_trading_rewards()` and `claim_lp_rewards()` update internal bookkeeping but never actually transfer MOLT tokens to the trader.

**Fix:**
- On `claim_trading_rewards()`: cross-contract call to `moltcoin.transfer(rewards_pool, trader, amount)`
- On `claim_lp_rewards()`: same pattern
- Need a rewards pool address (treasury or dedicated rewards wallet) that holds MOLT for distribution
- Add validation: claim fails if rewards pool has insufficient balance

### Task 2.7 — ClawPump: Real DEX Migration on Graduation

**File:** `contracts/clawpump/src/lib.rs` (1,109 lines)

**Problem:** When market cap hits 1,000,000 MOLT, the contract just sets a `graduated` flag and blocks further bonding curve trades. No actual migration to DEX occurs.

**Fix — on graduation, execute:**
1. Cross-contract call: `dex_governance.propose_new_pair(token_symbol, "MUSD")` (auto-approved for graduated tokens)
   OR directly: `dex_core.create_pair(token_symbol, "MUSD")` if admin-privileged
2. Cross-contract call: `dex_amm.create_pool(token_symbol, "MUSD", 30)` (30bps fee tier)
3. Seed initial liquidity:
   - The bonding curve has accumulated MOLT from buyers
   - Convert accumulated MOLT to mUSD equivalent (via oracle price or dex_core last price)
   - Deposit as initial liquidity in the AMM pool
4. Emit graduation event with pair address, pool address, initial liquidity

**Graduation stays in MOLT** — threshold is 1,000,000 MOLT market cap ($100K at $0.10). The conversion to mUSD pair happens only when creating the DEX listing.

---

## 6. Phase 3 — Security & Cross-Contract Wiring

### Task 3.1 — moltoracle: Secure Hash Function

**File:** `contracts/moltoracle/src/lib.rs` (857 lines)

**Problem:** `simple_hash()` uses LCG-style mixing (multiply + XOR) — trivially reversible. The VRF commit-reveal scheme relies on this hash being unpredictable.

**Fix:** Replace with a proper cryptographic hash. Options:
- Import a WASM-compatible SHA-256 or BLAKE2b implementation
- Or use the host's hashing function if exposed

### Task 3.2 — dex_governance: On-Chain Reputation Verification

**File:** `contracts/dex_governance/src/lib.rs` (667 lines)

**Problem:** `vote()` currently accepts caller-provided reputation value (capped at 2000). A malicious voter could claim maximum reputation.

**Fix:** Cross-contract call to `moltyid.get_reputation(voter_address)` to fetch verified on-chain reputation. Reject caller-provided values.

### Task 3.3 — bountyboard: Actual Token Transfer on Approval

**File:** `contracts/bountyboard/src/lib.rs` (715 lines)

**Problem:** `approve_work()` marks submitter as winner but doesn't transfer the bounty payment. Comment says "runtime to handle."

**Fix:** Cross-contract call to `moltcoin.transfer(bounty_creator, winner, bounty_amount)` or for mUSD bounties: `musd_token.transfer(...)`.

### Task 3.4 — clawvault: Real Yield Source Integration

**File:** `contracts/clawvault/src/lib.rs` (1,007 lines)

**Problem:** `harvest()` generates yield with hardcoded simulated APY rates (lending 3%, LP 5%, staking 8%). Not connected to real protocols.

**Fix:** Replace simulated yields with cross-contract calls:
- Lending strategy → `lobsterlend.deposit()` / `lobsterlend.withdraw()`
- LP strategy → `moltswap.add_liquidity()` / `moltswap.remove_liquidity()`
- Staking strategy → validator staking host functions

---

## 7. Phase 4 — Build, Test, Deploy

### Task 4.1 — Recompile All 26 Contract WASMs

```bash
cd contracts
for dir in */; do
  name="${dir%/}"
  cd "$name"
  cargo build --target wasm32-unknown-unknown --release
  cp target/wasm32-unknown-unknown/release/${name}.wasm ./${name}.wasm
  cd ..
done
```

**Validation:** Every WASM must be > 86 bytes. Expected sizes:
- Wrapped tokens: 5–8 KB (after export fix)
- DEX contracts: 8–30 KB
- Infrastructure: 9–45 KB

### Task 4.2 — Run All Rust Unit Tests

```bash
cargo test --workspace
```

**Target:** All 333+ tests pass, 0 failures.

Contract-specific test suites:
- musd_token: 16 tests
- wsol_token: 7 tests
- weth_token: 7 tests
- moltcoin: 6 tests
- moltpunks: 14 tests
- dex_rewards: multiple tests
- Plus all core/rpc/validator/p2p tests

### Task 4.3 — Clippy + Format Clean

```bash
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

Zero warnings, zero format drift.

### Task 4.4 — Integration Test: Full Reset + Boot

1. `bash ./reset-blockchain.sh`
2. Start 3 validators
3. Verify via RPC:
   - `getAllContracts` returns 26 contracts with non-empty ABIs
   - `getContractInfo("MOLT")` shows `mintable: false, burnable: true`
   - `getContractInfo("MUSD")` shows storage entries > 0 (initialized)
   - `getContractInfo("WSOL")` shows ABI with mint/burn/transfer functions
   - `getContractInfo("WETH")` shows ABI with mint/burn/transfer functions
   - Check dex_core has 3 pairs (MOLT/mUSD, WSOL/mUSD, WETH/mUSD)
   - Check dex_amm has 3 pools

### Task 4.5 — Explorer Verification

- All 26 contracts display correct ABI (no "No ABI available")
- Token Details show correct mintable/burnable per contract type
- Storage tab populates for initialized contracts
- Calls/Events populate as transactions occur

### Task 4.6 — Commit and Push

Single atomic commit with all changes:
```
feat: DEX completion milestone — tokenomics alignment + full contract initialization + enhancements

- Align genesis distribution to whitepaper (genesis.rs + website swapped validator/builder values)
- Rename ANNUAL_INFLATION_BPS → ANNUAL_REWARD_RATE_BPS (MOLT is not inflationary)
- Readjust deploy/upgrade fees and DEX parameters for launch price
- Fix wrapped token WASM exports (wsol/weth/musd: 86B → real WASMs)
- Genesis Phase 2: initialize all 26 contracts post-deploy
- Auto-list MOLT/mUSD, WSOL/mUSD, WETH/mUSD pairs at genesis
- dex_analytics: add 3d/1w/1y candle intervals (9 total)
- dex_margin: 100x leverage with tiered margin/penalty/funding
- dex_margin: host-level collateral locking (spendable→locked)
- dex_margin: governance-controlled insurance fund withdrawal
- dex_router: real cross-contract swap execution
- dex_rewards: actual MOLT transfer on reward claims (source: builder_grants)
- clawpump: real DEX migration on graduation (create pair + pool)
- moltoracle: replace insecure hash with cryptographic hash
- dex_governance: on-chain reputation verification via moltyid
- bountyboard: wire actual token transfer on work approval
- clawvault: connect to real yield sources (lobsterlend, moltswap)
- All 26 WASMs recompiled and verified
- 333+ tests pass, clippy clean, format clean
```

---

## 8. Contract Audit Summary

### All 26 Contracts — Current Status

| # | Contract | Source Lines | WASM Size | Status | Init Required |
|---|----------|-------------|-----------|--------|---------------|
| 1 | moltcoin | 320 | 5.4 KB | OK | Yes — sets admin, initial supply |
| 2 | musd_token | 881 | **86 B ⚠️** | BROKEN WASM | Yes — sets admin |
| 3 | wsol_token | 585 | **86 B ⚠️** | BROKEN WASM | Yes — sets admin |
| 4 | weth_token | 585 | **86 B ⚠️** | BROKEN WASM | Yes — sets admin |
| 5 | dex_core | 1,670 | 25 KB | OK | Yes — sets admin, pair count=0 |
| 6 | dex_amm | 1,229 | 8.0 KB | OK | Yes — sets admin, pool count=0 |
| 7 | dex_router | 861 | 8.0 KB | OK | Yes — sets admin + DEX addresses |
| 8 | dex_margin | 826 | 8.1 KB | OK | Yes — sets admin, insurance=0 |
| 9 | dex_rewards | 609 | 8.5 KB | OK | Yes — sets admin, reward rates |
| 10 | dex_governance | 667 | 8.0 KB | OK | Yes — sets admin, listing reqs |
| 11 | dex_analytics | 621 | 8.1 KB | OK | Yes — sets admin |
| 12 | clawpump | 1,109 | 17 KB | OK | Yes — sets admin, platform fees |
| 13 | moltswap | 1,175 | 5.6 KB | OK | Yes — sets admin |
| 14 | moltbridge | 1,921 | 17 KB | OK | Yes — sets admin, validator set |
| 15 | moltoracle | 857 | 17 KB | OK | Yes — sets admin |
| 16 | moltauction | 1,223 | 36 KB | OK | Yes — sets admin, marketplace fee |
| 17 | moltdao | 1,010 | 19 KB | OK | Yes — sets admin (initialize_dao) |
| 18 | moltmarket | 742 | 8.7 KB | OK | Yes — sets admin, marketplace fee |
| 19 | moltpunks | 500 | 9.2 KB | OK | Yes — sets admin |
| 20 | moltyid | 3,126 | 44 KB | OK | Yes — sets admin |
| 21 | lobsterlend | 1,203 | 12 KB | OK | Yes — sets admin, rates |
| 22 | reef_storage | 1,213 | 15 KB | OK | Yes — sets admin |
| 23 | bountyboard | 715 | 16 KB | OK | Yes — sets admin |
| 24 | compute_market | 1,687 | 17 KB | OK | Yes — sets admin |
| 25 | clawvault | 1,007 | 18 KB | OK | Yes — sets admin, fees |
| 26 | clawpay | 1,192 | 18 KB | OK | Yes — sets admin |

**Total source:** ~27,500 lines of Rust across 26 contracts  
**Broken WASMs:** 3 (wsol_token, weth_token, musd_token — export annotation issue)  
**Stubs:** 0 (all contracts have real implementation logic)

### Known Bugs to Fix

| Severity | Contract/File | Issue | Fix Task |
|----------|--------------|-------|----------|
| **CRITICAL** | genesis.rs + website | Genesis distribution swapped: validator rewards=250M, builder grants=150M (whitepaper says opposite) | 0.1 |
| **CRITICAL** | wsol/weth/musd_token | Missing `#[no_mangle] pub extern "C"` → 86-byte empty WASM | 1.1 |
| **CRITICAL** | All 26 contracts | Never initialized at genesis — empty storage, no admin | 1.2 |
| **HIGH** | consensus.rs | `ANNUAL_INFLATION_BPS` implies minting — MOLT is NOT inflationary | 0.2 |
| **HIGH** | processor.rs | Deploy/upgrade fees need readjustment for $0.10/MOLT ($2.50/$1.00 target) | 0.3 |
| **HIGH** | dex_core/dex_rewards/clawpump/dex_governance | Multiple parameter values misaligned for $0.10/MOLT | 0.3 |
| **HIGH** | dex_router | Swap execution is simulated — no real cross-contract calls | 2.5 |
| **HIGH** | dex_rewards | Reward claims are bookkeeping only — no MOLT transfer, no source wallet defined | 2.6 |
| **HIGH** | clawpump | Graduation just sets a flag — no DEX migration | 2.7 |
| **HIGH** | moltoracle | `simple_hash` is not cryptographic — VRF is vulnerable | 3.1 |
| **MEDIUM** | dex_margin | No host-level collateral locking — double-spend risk | 2.3 |
| **MEDIUM** | dex_margin | No insurance fund withdrawal mechanism | 2.4 |
| **MEDIUM** | dex_governance | Accepts caller-provided reputation (can be faked) | 3.2 |
| **MEDIUM** | dex_governance | `MIN_LISTING_LIQUIDITY` = 10 MOLT, comment says 10,000 MOLT (off by 1000×) | 0.3 |
| **MEDIUM** | bountyboard | `approve_work` doesn't transfer tokens | 3.3 |
| **MEDIUM** | clawvault | Yield generation is simulated with hardcoded APY | 3.4 |
| **LOW** | WHITEPAPER.md | Deploy fee listed as 0.0001 MOLT (stale — should be 2.5 MOLT) | 0.1 |

---

## 9. Genesis Initialization Order

The `genesis_auto_deploy()` function in `validator/src/main.rs` gains a Phase 2:

```
PHASE 1 — Deploy (existing)
  For each of 26 contracts:
    1. Read .wasm from disk
    2. Create ContractAccount (auto-extract ABI)
    3. Store account (put_account)
    4. Index in CF_PROGRAMS
    5. Register in symbol registry with metadata

PHASE 2 — Initialize (new)
  For each deployed contract, in dependency order:
    1. Instantiate WASM module via Wasmer
    2. Provide host imports (storage_read, storage_write, log, etc.)
    3. Call initialize(admin_pubkey) or call() dispatcher with init args
    4. Commit resulting storage writes to the ContractAccount
    5. Re-serialize and store updated account

PHASE 3 — Create Trading Pairs (new)
  1. Call dex_core.create_pair(MOLT_addr, MUSD_addr)
  2. Call dex_core.create_pair(WSOL_addr, MUSD_addr)
  3. Call dex_core.create_pair(WETH_addr, MUSD_addr)
  4. Call dex_amm.create_pool(MOLT_addr, MUSD_addr, 30)
  5. Call dex_amm.create_pool(WSOL_addr, MUSD_addr, 30)
  6. Call dex_amm.create_pool(WETH_addr, MUSD_addr, 30)
```

**Dependency graph:**

```
Layer 0 (no deps):    moltcoin, musd_token, wsol_token, weth_token
Layer 1 (tokens):     moltyid
Layer 2 (identity):   dex_core, dex_amm, moltswap
Layer 3 (DEX core):   dex_router (needs dex_core + dex_amm + moltswap addresses)
                      dex_margin, dex_rewards, dex_governance, dex_analytics
Layer 4 (protocols):  lobsterlend, moltbridge, moltoracle, moltdao
Layer 5 (apps):       moltmarket, moltpunks, moltauction, clawpump,
                      clawvault, clawpay, bountyboard, compute_market, reef_storage
```

---

## 10. Leverage Tier Table

For `dex_margin` contract — parameters scale with leverage to balance risk:

| Leverage | Initial Margin (bps) | Maintenance Margin (bps) | Liquidation Penalty (bps) | Funding Rate Multiplier | Risk Note |
|----------|---------------------|-------------------------|--------------------------|------------------------|-----------|
| ≤2x | 5000 (50%) | 2500 (25%) | 300 (3%) | 1.0x | Conservative |
| ≤3x | 3333 (33%) | 1700 (17%) | 300 (3%) | 1.0x | Conservative |
| ≤5x | 2000 (20%) | 1000 (10%) | 500 (5%) | 1.5x | Standard |
| ≤10x | 1000 (10%) | 500 (5%) | 500 (5%) | 2.0x | Aggressive |
| ≤25x | 400 (4%) | 200 (2%) | 700 (7%) | 3.0x | High risk |
| ≤50x | 200 (2%) | 100 (1%) | 1000 (10%) | 5.0x | Very high risk |
| ≤100x | 100 (1%) | 50 (0.5%) | 1500 (15%) | 10.0x | Maximum risk |

**How it works:**
- Trader requests 25x leverage on a 1000 mUSD position
- Notional value: 25,000 mUSD
- Initial margin required: 4% = 1000 mUSD (deposited and locked)
- Maintenance margin: 2% = 500 mUSD
- If position value drops such that remaining margin < 500 mUSD → liquidatable
- Liquidation penalty: 7% of notional = 1,750 mUSD (split: 875 to liquidator, 875 to insurance)
- Funding rate: 3x the base rate (charged every 8 hours)

**User can pick any leverage value up to 100x** — the system finds the matching tier. For example, 7x leverage uses the ≤10x tier parameters.

---

## 11. Candle Interval Table

For `dex_analytics` contract — 9 intervals covering seconds to years:

| # | Interval | Label | Period (seconds) | Max Candles | Retention | Storage Key Suffix |
|---|----------|-------|-----------------|-------------|-----------|-------------------|
| 1 | 1 minute | `1m` | 60 | 1,440 | 24 hours | `_60` |
| 2 | 5 minutes | `5m` | 300 | 2,016 | 7 days | `_300` |
| 3 | 15 minutes | `15m` | 900 | 2,880 | 30 days | `_900` |
| 4 | 1 hour | `1h` | 3,600 | 2,160 | 90 days | `_3600` |
| 5 | 4 hours | `4h` | 14,400 | 2,190 | 365 days | `_14400` |
| 6 | 1 day | `1d` | 86,400 | 1,095 | 3 years | `_86400` |
| 7 | 3 days | `3d` | 259,200 | 243 | 2 years | `_259200` |
| 8 | 1 week | `1w` | 604,800 | 260 | 5 years | `_604800` |
| 9 | 1 year | `1y` | 31,536,000 | unlimited | forever | `_31536000` |

**Candle record layout (48 bytes):**
| Field | Offset | Size | Type |
|-------|--------|------|------|
| open | 0 | 8 | u64 |
| high | 8 | 8 | u64 |
| low | 16 | 8 | u64 |
| close | 24 | 8 | u64 |
| volume | 32 | 8 | u64 |
| trades | 40 | 4 | u32 |
| timestamp | 44 | 4 | u32 |

---

## 12. Open Questions & Future Work

### Not in This Milestone (Deferred)

| Item | Reason |
|------|--------|
| Prediction Markets (Polymarket-style) | Separate milestone after DEX completion |
| Initial liquidity seeding from treasury | Requires economic design decisions |
| Auto-deleveraging (ADL) for dex_margin | Complex, insurance fund covers for now |
| Cross-chain bridge production security | Needs multi-sig validator set design |
| moltoracle mainnet hardening | Needs real cryptographic VRF (beyond hash fix) |
| Fiat onramp integration | External partnership |

### Answered Design Questions

| Question | Answer |
|----------|--------|
| What currency for launchpad? | **MOLT** — buy tokens with MOLT on bonding curve |
| What currency for DEX trading? | **mUSD** — all trading pairs are TOKEN/mUSD |
| How do users get MOLT? | Bridge USDT/USDC → mUSD, then swap mUSD → MOLT on DEX |
| How do users get mUSD? | Bridge USDT/USDC from external chains via moltbridge |
| Is MOLT mintable? | **No** — fixed 1B supply, deflationary via 40% fee burn |
| Is MOLT burnable? | **Yes** — fees burn mechanism |
| What happens on graduation? | Token gets DEX pair (TOKEN/mUSD), AMM pool created, initial liquidity seeded |
| Where does insurance fund live? | Counter in dex_margin contract storage, withdrawal via governance |
| Who can liquidate? | Anyone — bots incentivized by 50% of penalty |
| What is fee mining? | Traders earn MOLT proportional to fees paid (1:1 base × tier multiplier) |

---

## Execution Order

```
 PHASE 0 — Tokenomics & Distribution Alignment
 1. Task 0.1  — Fix genesis distribution mismatch (genesis.rs + website → whitepaper)
 2. Task 0.2  — Rename ANNUAL_INFLATION_BPS → ANNUAL_REWARD_RATE_BPS
 3. Task 0.3  — Readjust all MOLT parameters for $0.10/MOLT (LOCKED)

 PHASE 1 — Critical Fixes (Foundation)
 4. Task 1.1  — Fix 3 wrapped token exports + recompile WASMs
 5. Task 1.4  — mUSD as DEX quote currency enforcement

 PHASE 2 — DEX Contract Enhancements
 6. Task 2.1  — dex_analytics: add 3d/1w/1y candle intervals
 7. Task 2.2  — dex_margin: 100x leverage tiers
 8. Task 2.3  — dex_margin: host-level collateral locking
 9. Task 2.4  — dex_margin: insurance fund withdrawal
10. Task 2.5  — dex_router: real cross-contract calls
11. Task 2.6  — dex_rewards: actual MOLT transfer (source: builder_grants)
12. Task 2.7  — clawpump: real DEX migration on graduation

 PHASE 3 — Security & Cross-Contract Wiring
13. Task 3.1  — moltoracle: secure hash
14. Task 3.2  — dex_governance: on-chain reputation
15. Task 3.3  — bountyboard: token transfer
16. Task 3.4  — clawvault: real yield sources

 PHASE 1 (continued) — Genesis wiring (after all contracts enhanced)
17. Task 1.2  — Genesis initialization (all 26 contracts)
18. Task 1.3  — MOLT auto-listing at genesis (3 pairs + 3 pools)

 PHASE 4 — Build, Test, Deploy
19. Task 4.1  — Recompile all 26 WASMs
20. Task 4.2  — cargo test --workspace (333+ pass)
21. Task 4.3  — clippy + fmt clean
22. Task 4.4  — Integration test (reset + boot + verify)
23. Task 4.5  — Explorer verification
24. Task 4.6  — Commit and push
```

**Total: 24 tasks across 5 phases (0-4)**

---

*End of DEX Completion Milestone Plan*
