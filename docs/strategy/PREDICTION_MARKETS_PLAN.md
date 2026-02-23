# PredictionReef — Prediction Markets on MoltChain

**Date:** February 14, 2026
**Status:** Planning
**Branch:** `main`
**Depends on:** DEX Completion Milestone (complete), MoltOracle (deployed)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [How Polymarket Works (Reference Architecture)](#2-how-polymarket-works-reference-architecture)
3. [PredictionReef Architecture](#3-predictionreef-architecture)
4. [Contract Design — prediction_market](#4-contract-design--prediction_market)
5. [AMM Design — Binary CPMM](#5-amm-design--binary-cpmm)
6. [Multi-Outcome Markets](#6-multi-outcome-markets)
7. [Resolution via MoltOracle](#7-resolution-via-moltoracle)
8. [Settlement & Payout](#8-settlement--payout)
9. [Market Lifecycle](#9-market-lifecycle)
10. [Integration with Existing DEX](#10-integration-with-existing-dex)
11. [Governance & Safety](#11-governance--safety)
12. [Agent-First Design](#12-agent-first-design)
13. [Explorer / Wallet UI](#13-explorer--wallet-ui)
14. [Fee Structure & Economics](#14-fee-structure--economics)
15. [Storage Layout](#15-storage-layout)
16. [WASM Dispatch Table](#16-wasm-dispatch-table)
17. [Genesis Integration](#17-genesis-integration)
18. [Implementation Phases](#18-implementation-phases)
19. [Test Plan](#19-test-plan)
20. [Open Questions](#20-open-questions)

---

## 1. Executive Summary

PredictionReef brings Polymarket-class prediction markets to MoltChain as a first-party DEX feature — not a separate platform. Users trade outcome shares (YES/NO or multi-outcome) on real-world events, with prices reflecting live probabilities. Settlement in mUSD (the DEX quote currency), liquidity via an integrated AMM, and resolution through MoltOracle's attestation + VRF systems.

### Why This Matters

- **Prediction markets are the highest-engagement DeFi product** — Polymarket hit $1B+ monthly volume in 2024
- **Agent-first vision** — AI agents can programmatically trade on real-world events using data models, creating deep liquidity
- **Completes the DEX** — MoltChain DEX gets a "Predict" tab alongside Trade, Swap, Pool, and Earn
- **Leverages existing infrastructure** — MoltOracle for resolution, dex_analytics for charting, mUSD for settlement, MoltyID for reputation-gated market creation

### Key Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Settlement currency | **mUSD** | Consistent with DEX, stablecoin-denominated, $1.00 = 1 full share |
| AMM type | **Binary CPMM** (x·y=k) for 2-outcome; **Scalar CPMM** for multi | Simple, proven, gas-efficient |
| Resolution | **MoltOracle attestation** (multi-sig threshold) | Decentralized, already deployed, supports VRF |
| Share representation | **Contract storage balances** (not separate tokens) | No need for ERC-1155; MoltChain uses account-level balances |
| Market creation | **Reputation-gated** (500+ MoltyID rep) | Anti-spam, quality control, no separate governance vote needed |
| Dispute resolution | **Challenge period + bond + DAO escalation** | Prevents bad resolutions without blocking fast settlement |
| CLOB integration | **Optional** — AMM primary, CLOB for deep markets | Most markets use AMM; high-volume markets can also list on dex_core |

---

## 2. How Polymarket Works (Reference Architecture)

Understanding Polymarket's design to match and exceed it:

### Core Mechanics

1. **Conditional Tokens**: Every market creates YES and NO tokens (or multiple outcome tokens). Each full set of outcome tokens is backed by $1.00 USDC collateral.

2. **Minting**: A user deposits $1.00 USDC → receives 1 YES token + 1 NO token. This is the "mint" operation. The collateral is locked.

3. **Trading**: Users buy/sell outcome tokens on a CLOB (limit order book). Polymarket uses an off-chain orderbook with on-chain settlement (hybrid model).

4. **Pricing**: If YES trades at $0.65, the market implies a 65% probability. YES + NO always sum to $1.00 on the books (arbitrage enforces this).

5. **Resolution**: An oracle (Polymarket uses UMA's optimistic oracle) declares the outcome. Challenge period allows disputes.

6. **Redemption**: After resolution, winning shares are redeemed at $1.00 each. Losing shares are worthless. Collateral is distributed pro-rata.

### What We Keep

- Full collateralization (YES + NO backed by mUSD)
- Shares priced 0.00–1.00 mUSD
- Minting/redeeming share sets
- Resolution with dispute period
- Limit order trading for deep markets

### What We Improve

| Polymarket Limitation | PredictionReef Solution |
|-----------------------|------------------------|
| Off-chain orderbook (centralized matching) | Fully on-chain CPMM + optional CLOB |
| UMA oracle (single provider, slow disputes) | MoltOracle multi-sig attestation (threshold-based, faster) |
| No built-in AMM (relies on market makers) | Integrated CPMM ensures always-on liquidity |
| Separate platform from DEX | Integrated as a DEX tab — same wallet, same UI |
| No agent API | First-class agent support with MoltyID integration |
| ERC-1155 tokens (complex) | Simple balance storage (cheaper, faster) |
| External dispute resolution | On-chain DAO escalation via MoltDAO |

---

## 3. PredictionReef Architecture

### Single Contract Design

One contract — `prediction_market` — handles everything:
- Market creation and configuration
- Share minting and redemption
- Integrated CPMM AMM for each market
- Resolution and settlement
- Dispute handling

**Why single contract?** Cross-contract calls are stubs on MoltChain (return 0). Multi-contract designs would require multi-step transactions. A single contract keeps all state atomic and consistent.

### Component Map

```
┌─────────────────────────────────────────────────────┐
│                  prediction_market                   │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │  Market   │  │  Share   │  │  CPMM AMM Engine  │  │
│  │ Registry  │  │ Balances │  │  (per-market pool) │  │
│  └──────────┘  └──────────┘  └───────────────────┘  │
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │Resolution│  │ Dispute  │  │  Fee Collector &   │  │
│  │  Engine  │  │ Handler  │  │  LP Incentives     │  │
│  └──────────┘  └──────────┘  └───────────────────┘  │
│                                                      │
│  External reads:                                     │
│  ├─ MoltOracle: attestation verification             │
│  ├─ MoltyID: reputation check for market creation    │
│  └─ mUSD: balance reads for collateral verification  │
└─────────────────────────────────────────────────────┘
```

---

## 4. Contract Design — prediction_market

### Constants

```rust
// Market limits
const MAX_OUTCOMES: u8 = 8;           // Max outcomes per market (2 for binary, up to 8 for multi)
const MAX_MARKETS: u64 = 100_000;     // Global market cap
const MAX_OPEN_MARKETS: u64 = 10_000; // Concurrently active markets
const MIN_COLLATERAL: u64 = 1_000_000; // 1 mUSD minimum (6 decimals)
const MAX_COLLATERAL: u64 = 100_000_000_000; // 100K mUSD max per market

// Timing (in slots; 1 slot ≈ 0.5s)
const MIN_DURATION: u64 = 7_200;       // 1 hour minimum market duration
const MAX_DURATION: u64 = 63_072_000;  // 1 year maximum
const RESOLUTION_TIMEOUT: u64 = 604_800; // 7 days to resolve after close
const DISPUTE_PERIOD: u64 = 172_800;    // 48 hours to challenge resolution
const EMERGENCY_TIMEOUT: u64 = 2_592_000; // 30 days — auto-void if unresolved

// Fees (basis points)
const MARKET_CREATION_FEE: u64 = 10_000_000; // 10 mUSD (anti-spam)
const TRADING_FEE_BPS: u64 = 200;   // 2% on AMM swaps
const RESOLUTION_REWARD_BPS: u64 = 50; // 0.5% of pool to resolver
const LP_FEE_BPS: u64 = 100;          // 1% to liquidity providers

// AMM
const INITIAL_LIQUIDITY: u64 = 1_000; // Minimum initial liquidity per outcome (in shares)
const MIN_SHARE_PRICE: u64 = 10_000;  // $0.01 minimum (6 decimal mUSD)
const MAX_SHARE_PRICE: u64 = 990_000; // $0.99 maximum (prevents riskless trades at edges)

// Reputation
const MIN_REPUTATION_CREATE: u64 = 500;  // MoltyID rep to create markets
const MIN_REPUTATION_RESOLVE: u64 = 1000; // MoltyID rep to submit resolution
const DISPUTE_BOND: u64 = 100_000_000;    // 100 mUSD bond to dispute (refunded if upheld)
```

### Market States

```
PENDING    → Market created, accepting initial liquidity (before trading opens)
ACTIVE     → Trading open, AMM active
CLOSED     → Trading closed (time expired), awaiting resolution
RESOLVING  → Resolution submitted, in dispute period
RESOLVED   → Final — winning outcome determined, redemptions enabled
DISPUTED   → Resolution challenged, escalated to DAO vote or re-attestation
VOIDED     → Market cancelled — all collateral returned at original mint ratios
```

State transitions:
```
PENDING ──(add_initial_liquidity)──► ACTIVE
ACTIVE  ──(close_time reached)────► CLOSED
ACTIVE  ──(emergency_void)────────► VOIDED
CLOSED  ──(submit_resolution)─────► RESOLVING
CLOSED  ──(timeout 30d)──────────► VOIDED
RESOLVING ─(dispute_period_pass)──► RESOLVED
RESOLVING ─(challenge_resolution)─► DISPUTED
DISPUTED ──(dao_resolve)──────────► RESOLVED
DISPUTED ──(dao_void)────────────► VOIDED
RESOLVED ──(redeem_shares)────────► (payouts)
VOIDED   ──(reclaim_collateral)───► (refunds)
```

### Data Structures

**Market Record (192 bytes)**
```
Bytes 0..8     : market_id (u64)
Bytes 8..40    : creator (Pubkey)
Bytes 40..48   : created_slot (u64)
Bytes 48..56   : close_slot (u64)
Bytes 56..64   : resolve_slot (u64) — when resolution submitted
Byte  64       : status (u8) — enum MarketStatus
Byte  65       : outcome_count (u8) — 2 for binary, up to 8
Byte  66       : winning_outcome (u8) — 0xFF = unresolved
Byte  67       : category (u8) — 0=politics, 1=sports, 2=crypto, 3=science, 4=entertainment, 5=economics, 6=tech, 7=custom
Bytes 68..76   : total_collateral (u64) — total mUSD locked
Bytes 76..84   : total_volume (u64) — cumulative trading volume
Bytes 84..92   : resolution_bond (u64) — resolver's bond
Bytes 92..124  : resolver (Pubkey) — who submitted resolution
Bytes 124..156 : question_hash (32 bytes) — SHA-256 of question text
Bytes 156..164 : dispute_end_slot (u64) — when dispute period ends
Bytes 164..172 : fees_collected (u64) — total fees earned
Bytes 172..180 : lp_total_shares (u64) — total LP shares for AMM
Bytes 180..188 : oracle_attestation_hash (32 bytes, first 8) — link to MoltOracle
Bytes 188..192 : pad (4 bytes)
```

**Outcome Pool (64 bytes per outcome)**
```
Bytes 0..8     : reserve (u64) — AMM virtual reserve for this outcome
Bytes 8..16    : total_shares (u64) — total shares minted for this outcome
Bytes 16..24   : total_redeemed (u64) — shares redeemed after resolution
Bytes 24..32   : price_last (u64) — last traded price (6 decimal mUSD basis)
Bytes 32..40   : volume (u64) — outcome-specific volume
Bytes 40..48   : open_interest (u64) — outstanding unredeemed shares
Bytes 48..56   : pad (8 bytes)
Bytes 56..64   : pad (8 bytes)
```

**User Position (16 bytes per user per outcome per market)**
```
Bytes 0..8     : shares (u64) — shares held
Bytes 8..16    : cost_basis (u64) — total mUSD spent acquiring
```

---

## 5. AMM Design — Binary CPMM

### Constant Product Market Maker

For binary markets (YES/NO), we use x·y=k where:
- `x` = virtual reserve of YES tokens in the pool
- `y` = virtual reserve of NO tokens in the pool
- `k` = x·y (invariant, increases with liquidity additions)

**Price derivation:**
```
price_YES = y / (x + y)
price_NO  = x / (x + y)
price_YES + price_NO = 1.00  (always, by construction)
```

**Example:** If pool has 1000 YES and 1000 NO reserves:
- price_YES = 1000 / 2000 = $0.50 (50% probability)
- price_NO  = 1000 / 2000 = $0.50

After someone buys 100 YES shares for mUSD:
```
New x (YES reserve) decreases, y (NO reserve) stays or increases
Buying YES → price_YES increases (more likely)
```

### Buy Operation (buy_shares)

```
Input: market_id, outcome, amount_musd
1. Mint a virtual complete set: amount_musd → (1 YES + 1 NO) * amount
2. Sell unwanted outcome shares into pool
3. Net result: user receives shares of desired outcome
4. Pool absorbs opposite outcome shares, adjusting prices

Detailed math:
  shares_per_set = amount_musd / 1_000_000  (1 mUSD = 1 complete set)
  
  // User wants outcome A. We sell outcome B shares into pool.
  b_shares_to_sell = shares_per_set
  
  // CPMM: selling B shares into pool, receiving A shares out
  // x * y = k
  // new_y = y + b_shares_to_sell
  // new_x = k / new_y
  // a_shares_received = x - new_x
  
  a_shares_received = x * b_shares_to_sell / (y + b_shares_to_sell)
  
  // Total shares user gets: shares_per_set (from mint) + a_shares_received (from swap)
  total_shares = shares_per_set + a_shares_received
  
  // Fee: deducted from a_shares_received
  fee_shares = a_shares_received * TRADING_FEE_BPS / 10_000
  actual_shares = total_shares - fee_shares
```

### Sell Operation (sell_shares)

```
Input: market_id, outcome, shares_amount
1. Sell outcome shares into pool for opposite outcome shares
2. Burn complete sets (1 YES + 1 NO) → 1 mUSD
3. Return mUSD to user

Math:
  // Selling A shares into pool for B shares
  b_received = y * shares_to_sell / (x + shares_to_sell)
  
  // Burn min(shares_remaining, b_received) complete sets
  sets_to_burn = min(a_shares_remaining_after_swap, b_received)
  musd_returned = sets_to_burn * 1_000_000
  
  fee = musd_returned * TRADING_FEE_BPS / 10_000
  net_payout = musd_returned - fee
```

### Mint Complete Set

```
Input: market_id, amount_musd
1. Lock amount_musd as collateral
2. Mint `amount` shares of EVERY outcome (1 YES + 1 NO for binary)
3. User holds a hedged position (worth exactly amount_musd before fees)
```

### Redeem Complete Set

```
Input: market_id, amount
1. Burn `amount` shares of EVERY outcome
2. Return `amount * 1_000_000` mUSD (collateral release)
3. Zero-fee operation (no price impact, pure collateral unlock)
```

### Initial Liquidity Provision

Market creator provides initial liquidity:
```
Input: market_id, musd_amount, initial_odds (optional)
1. Mint shares: musd_amount → N complete sets
2. Initialize pool reserves based on initial_odds
   Default (50/50): x = N, y = N → k = N²
   Custom (e.g., 70/30): x = N * 0.3, y = N * 0.7  (more YES expected)
3. Creator receives LP tokens proportional to liquidity
```

---

## 6. Multi-Outcome Markets

For markets with 3-8 outcomes (e.g., "Who wins the election?" with 5 candidates), we extend CPMM:

### Scalar CPMM (Product of reserves)

```
k = r₁ * r₂ * r₃ * ... * rₙ
price_i = (product of all OTHER reserves) / (sum of all such products)
```

Simplified: for N outcomes, each outcome's probability is:
```
price_i = (1 / reserve_i) / sum(1 / reserve_j for all j)
```

All prices sum to 1.00 by construction.

### Trade Math (Multi-Outcome)

Buying outcome i:
```
1. Mint 1 complete set per mUSD deposited (all N outcome shares)
2. Sell all non-desired outcome shares into pool
3. For each non-desired outcome j:
   shares_received_i += reserve_i * shares_j_sold / (reserve_j + shares_j_sold)
```

This is more gas-intensive but capped at 8 outcomes max.

---

## 7. Resolution via MoltOracle

### Resolution Flow

```
1. MARKET CLOSES (close_slot reached)
   Trading stops, AMM frozen

2. RESOLUTION SUBMISSION (anyone with 1000+ reputation)
   - Resolver calls submit_resolution(market_id, winning_outcome, attestation_hash)
   - Posts a bond (100 mUSD) — slashed if resolution is successfully challenged
   - Resolution references a MoltOracle attestation (multi-sig evidence hash)
   
3. ORACLE VERIFICATION
   - Contract reads MoltOracle attestation via storage key
   - Verifies attestation has >= 3 signatories (RESOLUTION_THRESHOLD)
   - Verifies attestation data_hash matches the market's question + outcome
   
4. DISPUTE PERIOD (48 hours)
   - Anyone can call challenge_resolution(market_id, evidence_hash, bond)
   - Challenger posts 100 mUSD bond
   - Market enters DISPUTED state
   
5a. NO DISPUTE → RESOLVED
   - After 48h, resolution is finalized
   - Resolver gets their bond back + 0.5% resolution reward
   - Winning shares redeemable at $1.00 each
   
5b. DISPUTE → DAO ESCALATION
   - MoltDAO governance vote (72h voting period)
   - Options: CONFIRM original resolution | OVERRIDE with different outcome | VOID market
   - Losing party's bond is distributed: 50% to winner, 50% to DAO treasury
```

### Oracle Attestation Format

MoltOracle's existing `submit_attestation()` and `verify_attestation()` are reused:

```
Attestation data layout for prediction market resolution:
  Bytes 0..8    : market_id (u64 LE)
  Byte  8       : winning_outcome (u8)
  Bytes 9..17   : timestamp (u64 LE) — event occurrence time
  Bytes 17..49  : evidence_hash (32 bytes) — SHA-256 of evidence URL/document
```

Multiple independent oracles (3+ required) submit attestations with the same data_hash. When threshold is met, the market can accept the resolution.

### Resolution Types

| Type | Resolution Method | Example |
|------|------------------|---------|
| Binary (YES/NO) | Oracle attestation | "Will BTC hit $100K by March?" |
| Multi-outcome | Oracle attestation | "Which AI model wins benchmark X?" |
| Price-based | MoltOracle price feed | "ETH price > $5000 on March 1?" |
| VRF-based | MoltOracle commit-reveal | "What color is drawn in lottery round 42?" |
| Time-based | Automatic at close_slot | "Total MOLT burned by end of epoch?" (on-chain data) |

---

## 8. Settlement & Payout

### Redemption After Resolution

```
redeem_shares(market_id, outcome)
1. Verify market status == RESOLVED
2. If outcome == winning_outcome:
     payout = user_shares * 1_000_000  (each share = 1 mUSD)
     credit mUSD to user balance
3. If outcome != winning_outcome:
     payout = 0 (shares worthless)
4. Clear user's position for this market/outcome
```

### Voided Market Refund

```
reclaim_collateral(market_id)
1. Verify market status == VOIDED
2. Calculate user's total cost basis across all outcomes
3. Refund pro-rata based on cost_basis / total_collateral
4. Clear all positions
```

### LP Withdrawal

```
withdraw_liquidity(market_id, lp_shares)
1. Can only withdraw from ACTIVE markets (not CLOSED/RESOLVED)
2. Calculate proportional share of pool reserves
3. Return mUSD collateral minus pool imbalance
4. Burn LP shares
```

---

## 9. Market Lifecycle

### Complete Lifecycle Example

```
Day 0: Creator with 500+ reputation creates market
        "Will SOL reach $250 by March 31, 2026?"
        Duration: 45 days
        Category: Crypto
        Initial liquidity: 500 mUSD (50/50 odds)
        Creation fee: 10 mUSD
        
        Market state: PENDING → ACTIVE (once liquidity added)

Day 1-44: Trading
        Users buy/sell YES and NO shares
        AMM provides continuous liquidity
        Price fluctuates based on market sentiment
        Analytics track volume, price history, open interest
        
        State: ACTIVE

Day 45: Market closes at end of March 31
        Trading stops, AMM frozen
        
        State: CLOSED

Day 45-52: Resolution window (7 days)
        3+ oracle nodes attest: "SOL did NOT reach $250"
        Resolver (1000+ rep) submits resolution: winning_outcome = NO
        Posts 100 mUSD bond
        
        State: RESOLVING

Day 52-54: Dispute period (48 hours)
        No challenges filed
        
        State: RESOLVING → RESOLVED

Day 54+: Settlement
        YES holders: shares worth $0.00
        NO holders: redeem each share for $1.00 mUSD
        Resolver gets bond back + 0.5% of total collateral
        
        State: RESOLVED (permanent)
```

---

## 10. Integration with Existing DEX

### Single Platform Architecture

PredictionReef is NOT a separate app — it's a new tab in the DEX interface:

```
┌─────────────────────────────────────────┐
│           MoltChain DEX                 │
│                                         │
│  [Trade] [Swap] [Pool] [Earn] [Predict] │
│                                         │
│  ┌─────────────────────────────────┐    │
│  │      Prediction Markets         │    │
│  │                                 │    │
│  │  Featured    Categories    My   │    │
│  │   Markets    & Search    Bets   │    │
│  │                                 │    │
│  │  ┌───────────────────────┐      │    │
│  │  │ "Will BTC hit $100K?" │      │    │
│  │  │  YES: $0.72  NO: $0.28│      │    │
│  │  │  Vol: 45K mUSD        │      │    │
│  │  │  [Buy YES] [Buy NO]   │      │    │
│  │  └───────────────────────┘      │    │
│  └─────────────────────────────────┘    │
└─────────────────────────────────────────┘
```

### Cross-Module Integration Points

| Existing Module | Integration |
|----------------|-------------|
| **dex_analytics** | Track prediction market volume, price candles per market (reuse candle system for outcome share prices) |
| **dex_governance** | DAO dispute resolution votes (new proposal type: RESOLVE_PREDICTION = 4) |
| **dex_rewards** | Trading rewards for prediction market volume (fee mining applies same as DEX trades) |
| **MoltOracle** | Resolution attestations + price feed markets + VRF for lottery-type markets |
| **MoltyID** | Reputation gates for market creation (500+) and resolution submission (1000+) |
| **mUSD** | Settlement currency — all collateral, payouts, and fees in mUSD |
| **dex_core** | Optional CLOB listing for high-volume prediction markets (outcome shares as tradeable "tokens") |
| **dex_amm** | NOT used directly — prediction markets have their own AMM (CPMM, simpler than concentrated liquidity) |

### Optional CLOB Integration for Deep Markets

When a prediction market reaches high volume (>10K mUSD), it may benefit from a CLOB:

```
1. Admin/governance calls create_clob_market(prediction_market_id)
2. Creates virtual token pair on dex_core: YES_TOKEN/mUSD
3. Traders can place limit orders for outcome shares
4. AMM continues to operate in parallel (provides baseline liquidity)
5. On resolution, all open CLOB orders are cancelled and settled
```

This is optional and can be added in a later phase.

---

## 11. Governance & Safety

### Market Creation Controls

| Guard | Mechanism |
|-------|-----------|
| Anti-spam | 10 mUSD creation fee + 500 reputation minimum |
| Content moderation | Category system + admin/DAO can flag/delist markets |
| Duplicate prevention | Question hash prevents identical markets |
| Market cap | MAX_OPEN_MARKETS = 10,000 prevents storage bloat |
| Duration limits | Min 1 hour, max 1 year |

### Resolution Safety

| Guard | Mechanism |
|-------|-----------|
| Oracle threshold | 3+ independent attestations required |
| Resolver bond | 100 mUSD at risk — slashed for incorrect resolution |
| Challenge mechanism | 48-hour dispute window with bond |
| DAO backstop | MoltDAO can override any resolution |
| Timeout void | Market auto-voids after 30 days if unresolved |
| Emergency halt | Admin pause for critical issues |

### Circuit Breakers

```
- If any single market exceeds 50K mUSD collateral → require admin approval for additional deposits
- If total platform collateral exceeds 1M mUSD → governance review triggered
- If AMM price moves >50% in a single slot → 60-second trading pause per market
- If resolution is challenged 3+ times → auto-escalate to DAO
```

---

## 12. Agent-First Design

### Why Agents Love Prediction Markets

Prediction markets are the most natural DeFi product for AI agents:
- Agents can scrape news, analyze data, build models → trade on probabilities
- API-first interface (no UI needed)
- Markets on everything — crypto, sports, politics, science, tech
- Continuous re-pricing as new information arrives — reward fast data processing

### Agent API Patterns

All operations are WASM callable (opcode dispatch):

```
// Agent creates market based on data analysis
call(opcode=1, creator, category, close_slot, question_hash, outcomes, initial_musd)

// Agent buys shares based on probability model
call(opcode=4, trader, market_id, outcome, amount_musd)

// Agent reads market price (no gas cost in read-only mode)
call(opcode=20, market_id, outcome) → price_in_musd

// Agent can automatically resolve markets via oracle attestation
submit_attestation → submit_resolution flow
```

### Agent Reputation Loop

```
Agent registers identity → Earns reputation via trading → 
Unlocks market creation → Creates quality markets → 
Resolves markets accurately → Earns resolution rewards →
Higher reputation → Access to more features
```

This creates a positive flywheel: better agents create better markets.

---

## 13. Explorer / Wallet UI

### Predict Tab Layout

The `Predict` tab in the DEX shows:

**Top Section — Featured Markets**
```
┌─ Featured Markets ──────────────────────────────────────┐
│                                                          │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐       │
│  │  🏛 Politics  │ │  📈 Crypto   │ │  ⚽ Sports   │       │
│  │ "Next POTUS" │ │"BTC >$100K" │ │"NBA Finals" │       │
│  │ YES: $0.62   │ │ YES: $0.81  │ │ YES: $0.34  │       │
│  │ Vol: 234K    │ │ Vol: 89K    │ │ Vol: 156K   │       │
│  └─────────────┘ └─────────────┘ └─────────────┘       │
└─────────────────────────────────────────────────────────┘
```

**Market Browse — Categories**
```
[All] [Politics] [Crypto] [Sports] [Science] [Entertainment] [Economics] [Tech]

Sort by: [Volume ▼] [Newest] [Ending Soon] [Most Popular]

┌───────────────────────────────────────────────────────┐
│ "Will ETH reach $5,000 by June 2026?"                │
│ Category: Crypto          Expires: Jun 30, 2026      │
│ YES: $0.45  NO: $0.55    Volume: 12,340 mUSD        │
│ Open Interest: 8,500 mUSD   Created by: agent.molt   │
│ [Buy YES] [Buy NO] [Details →]                       │
├───────────────────────────────────────────────────────┤
│ "Next US Fed Rate Decision: Cut, Hold, or Hike?"     │
│ Category: Economics       Expires: Mar 19, 2026      │
│ Cut: $0.82  Hold: $0.15  Hike: $0.03 (3 outcomes)  │
│ Volume: 67,800 mUSD         Created by: macro_bot    │
│ [Trade →] [Details →]                                │
└───────────────────────────────────────────────────────┘
```

**Market Detail Page**
```
┌────────────────────────────────────────────────────┐
│ "Will BTC reach $100K by March 2026?"              │
│ Category: Crypto  │  Status: ACTIVE                │
│ Created: Feb 14   │  Closes: Mar 31, 2026          │
├────────────────────────────────────────────────────┤
│                                                    │
│  [Price Chart — candles for YES share price]       │
│  (reuses dex_analytics candle rendering)           │
│                                                    │
├──────────────────┬─────────────────────────────────┤
│   BUY PANEL      │    MARKET STATS                 │
│                  │                                 │
│  ○ YES ($0.72)   │  Volume (24h): 4,500 mUSD      │
│  ○ NO  ($0.28)   │  Total Volume: 89,200 mUSD     │
│                  │  Open Interest: 34,500 mUSD     │
│  Amount: [___]   │  Liquidity: 12,000 mUSD        │
│  Est. Return:    │  Traders: 234                   │
│   $1.39 per $1   │  Created by: oracle_agent.molt  │
│                  │                                 │
│  [Buy Shares]    │  Resolution: MoltOracle         │
│  [Sell Shares]   │  Oracle Attestations: 0/3       │
└──────────────────┴─────────────────────────────────┘
│                                                    │
│  MY POSITIONS                                      │
│  YES: 250 shares (avg cost: $0.65)  P/L: +$17.50  │
│  [Sell] [Redeem After Resolution]                  │
│                                                    │
│  ORDER BOOK (if CLOB enabled)                      │
│  Bid: $0.71 (500)  │  Ask: $0.73 (300)            │
└────────────────────────────────────────────────────┘
```

**My Bets Tab**
```
Active Positions  │  Resolved  │  History

┌─ Active ──────────────────────────────────────────┐
│ Market                 │ Position │ P/L    │ Value │
│ "BTC > $100K"          │ 250 YES  │ +$17   │ $180  │
│ "Next Fed Decision"    │ 100 CUT  │ -$5    │ $82   │
│ "SOL > $250"           │ 500 NO   │ +$65   │ $375  │
└────────────────────────────────────────────────────┘
```

---

## 14. Fee Structure & Economics

### Fee Distribution

| Fee Type | Amount | Distribution |
|----------|--------|-------------|
| Market creation | 10 mUSD flat | 100% to protocol treasury |
| AMM trading | 2% of trade value | 50% LPs / 30% protocol / 20% stakers |
| Resolution reward | 0.5% of total collateral | 100% to resolver |
| Dispute bond (loser) | 100 mUSD | 50% to winner / 50% to DAO treasury |
| Mint/redeem complete sets | 0% | No fee (pure collateral lock/unlock) |

### Revenue Projections

At 10% of Polymarket's volume (~$100M/month in peak):
```
Monthly volume: $10M mUSD
Trading fees (2%): $200K mUSD
  → Protocol share (30%): $60K mUSD
  → LP share (50%): $100K mUSD
  → Staker share (20%): $40K mUSD
Creation fees: ~50 markets × 10 mUSD = $500 mUSD
Resolution rewards: ~$50K × 0.5% = $250 mUSD
```

### LP Incentives

Liquidity providers earn:
1. **Trading fees** — 50% of the 2% AMM fee
2. **Resolution bonus** — proportional share of remaining pool excess on resolution
3. **Mining rewards** — via dex_rewards integration (same as DEX LP rewards)

LP risk: impermanent loss if the market resolves to an extreme outcome (all YES or all NO). This is inherent to prediction market LPs.

---

## 15. Storage Layout

### Global State

| Key | Type | Description |
|-----|------|-------------|
| `pm_admin` | `[u8; 32]` | Admin pubkey |
| `pm_market_count` | `u64` | Global market counter |
| `pm_open_markets` | `u64` | Currently active markets |
| `pm_total_volume` | `u64` | Platform lifetime volume |
| `pm_total_collateral` | `u64` | Current total collateral locked |
| `pm_paused` | `u8` | Emergency pause flag |
| `pm_reentrancy` | `u8` | Reentrancy guard |
| `pm_fees_collected` | `u64` | Platform fees accumulated |
| `pm_moltyid_addr` | `[u8; 32]` | MoltyID contract address |
| `pm_oracle_addr` | `[u8; 32]` | MoltOracle contract address |
| `pm_musd_addr` | `[u8; 32]` | mUSD token contract address |
| `pm_dex_gov_addr` | `[u8; 32]` | DEX governance address (for disputes) |

### Per-Market State

| Key | Type | Description |
|-----|------|-------------|
| `pm_m_{id}` | `[u8; 192]` | Market record |
| `pm_q_{id}` | `Vec<u8>` (up to 512) | Question text (UTF-8) |
| `pm_o_{id}_{outcome}` | `[u8; 64]` | Outcome pool data |
| `pm_on_{id}_{outcome}` | `Vec<u8>` (up to 64) | Outcome name/label |

### Per-User State

| Key | Type | Description |
|-----|------|-------------|
| `pm_p_{id}_{addr_hex}_{outcome}` | `[u8; 16]` | Position (shares + cost_basis) |
| `pm_lp_{id}_{addr_hex}` | `u64` | LP shares for market |

### Indices

| Key | Type | Description |
|-----|------|-------------|
| `pm_cat_{category}_{idx}` | `u64` | Market ID by category index |
| `pm_catc_{category}` | `u64` | Count per category |
| `pm_active_{idx}` | `u64` | Active market IDs (for frontpage) |
| `pm_user_{addr_hex}_{idx}` | `u64` | Market IDs user participated in |
| `pm_userc_{addr_hex}` | `u64` | User's market participation count |

---

## 16. WASM Dispatch Table

Single `call()` entry point, opcode in `args[0]`:

| Opcode | Function | Args Layout | Returns |
|--------|----------|-------------|---------|
| 0x00 | `initialize` | `[admin 32B]` | u32 (1=ok) |
| 0x01 | `create_market` | `[creator 32B][category 1B][close_slot 8B][outcome_count 1B][question_hash 32B][question_ptr offset 4B][question_len 4B]` | u32 (market_id) |
| 0x02 | `add_initial_liquidity` | `[provider 32B][market_id 8B][amount_musd 8B][odds_bps array (2B × outcomes)]` | u32 (1=ok) |
| 0x03 | `add_liquidity` | `[provider 32B][market_id 8B][amount_musd 8B]` | u32 (lp_shares) |
| 0x04 | `buy_shares` | `[trader 32B][market_id 8B][outcome 1B][amount_musd 8B]` | u32 (shares_received) |
| 0x05 | `sell_shares` | `[trader 32B][market_id 8B][outcome 1B][shares 8B]` | u32 (musd_received) |
| 0x06 | `mint_complete_set` | `[user 32B][market_id 8B][amount 8B]` | u32 (1=ok) |
| 0x07 | `redeem_complete_set` | `[user 32B][market_id 8B][amount 8B]` | u32 (musd_returned) |
| 0x08 | `submit_resolution` | `[resolver 32B][market_id 8B][winning_outcome 1B][attestation_hash 32B][bond 8B]` | u32 (1=ok) |
| 0x09 | `challenge_resolution` | `[challenger 32B][market_id 8B][evidence_hash 32B][bond 8B]` | u32 (1=ok) |
| 0x0A | `finalize_resolution` | `[caller 32B][market_id 8B]` | u32 (1=ok) |
| 0x0B | `dao_resolve` | `[caller 32B][market_id 8B][winning_outcome 1B]` | u32 (1=ok, admin/DAO only) |
| 0x0C | `dao_void` | `[caller 32B][market_id 8B]` | u32 (1=ok, admin/DAO only) |
| 0x0D | `redeem_shares` | `[user 32B][market_id 8B][outcome 1B]` | u32 (musd_payout) |
| 0x0E | `reclaim_collateral` | `[user 32B][market_id 8B]` | u32 (musd_refund) |
| 0x0F | `withdraw_liquidity` | `[provider 32B][market_id 8B][lp_shares 8B]` | u32 (musd_returned) |
| 0x10 | `emergency_pause` | `[caller 32B]` | u32 |
| 0x11 | `emergency_unpause` | `[caller 32B]` | u32 |
| 0x12 | `set_moltyid_address` | `[caller 32B][address 32B]` | u32 |
| 0x13 | `set_oracle_address` | `[caller 32B][address 32B]` | u32 |
| **Queries** | | | |
| 0x20 | `get_market` | `[market_id 8B]` | return_data: 192B market record |
| 0x21 | `get_outcome_pool` | `[market_id 8B][outcome 1B]` | return_data: 64B pool |
| 0x22 | `get_price` | `[market_id 8B][outcome 1B]` | return_data: u64 price |
| 0x23 | `get_position` | `[market_id 8B][addr 32B][outcome 1B]` | return_data: 16B position |
| 0x24 | `get_market_count` | `[]` | return_data: u64 |
| 0x25 | `get_user_markets` | `[addr 32B]` | return_data: u64 count |
| 0x26 | `quote_buy` | `[market_id 8B][outcome 1B][amount_musd 8B]` | return_data: u64 shares |
| 0x27 | `quote_sell` | `[market_id 8B][outcome 1B][shares 8B]` | return_data: u64 musd |
| 0x28 | `get_pool_reserves` | `[market_id 8B]` | return_data: u64[] reserves |

---

## 17. Genesis Integration

### GENESIS_CONTRACT_CATALOG Entry

```rust
("prediction_market", "PREDICT", "PredictionReef", "prediction"),
```

### Initialization

```
function: "call"
args: [0x00][admin 32B]  (opcode dispatch init)
```

### Post-Init Configuration

```
set_moltyid_address → MoltyID contract address
set_oracle_address → MoltOracle contract address
```

### Genesis Markets (Optional)

Could auto-create 1-2 featured markets at genesis for demo:
```
"Will MoltChain reach 1000 validators in 2026?" (YES/NO, closes Dec 31 2026)
"MOLT price prediction Q2 2026: >$0.50?" (YES/NO, closes Jun 30 2026)
```

---

## 18. Implementation Phases

### Phase A — Core Contract (Est. 2000-2500 lines)

```
A.1  Contract skeleton: dispatch table, admin, pause, reentrancy
A.2  Market creation: create_market, question storage, category indexing
A.3  Binary CPMM AMM: buy_shares, sell_shares, mint/redeem complete sets
A.4  Market lifecycle: PENDING → ACTIVE → CLOSED state machine
A.5  Unit tests: AMM math, market creation, state transitions
```

### Phase B — Resolution & Settlement

```
B.1  Resolution submission + bond posting
B.2  Oracle attestation verification (read MoltOracle storage)
B.3  Dispute mechanism + challenge bonds
B.4  Finalization + winning share redemption
B.5  Voided market refunds
B.6  Unit tests: resolution flows, edge cases, dispute scenarios
```

### Phase C — Multi-Outcome & Advanced

```
C.1  Extend CPMM for 3-8 outcome markets
C.2  Liquidity provision and withdrawal
C.3  Fee collection and distribution
C.4  MoltyID reputation gating
C.5  User position tracking and history
C.6  Unit tests: multi-outcome math, LP mechanics
```

### Phase D — Integration & UI

```
D.1  Add to GENESIS_CONTRACT_CATALOG + initialization
D.2  Compile WASM + add to genesis deploy
D.3  RPC endpoints: getPredictionMarkets, getMarketInfo, getUserPredictions
D.4  Explorer: Predict tab page (market browse + detail)
D.5  Wallet: My Bets section in address.html
D.6  dex_analytics integration: candle data for outcome share prices
D.7  dex_rewards integration: fee mining for prediction market trades
```

### Phase E — Polish & Launch

```
E.1  dex_governance: add RESOLVE_PREDICTION proposal type
E.2  Circuit breakers and safety limits
E.3  Full integration test suite
E.4  Agent SDK documentation
E.5  Create 2-3 featured genesis markets
```

---

## 19. Test Plan

### Unit Tests (~80+ tests)

**AMM Math (20 tests)**
```
test_binary_cpmm_initial_prices
test_binary_cpmm_buy_yes_moves_price_up
test_binary_cpmm_buy_no_moves_price_down
test_prices_always_sum_to_one
test_large_buy_extreme_price
test_sell_reverse_of_buy
test_mint_complete_set_no_price_impact
test_redeem_complete_set_returns_collateral
test_buy_fee_deduction
test_sell_fee_deduction
test_multi_outcome_prices_sum_to_one
test_multi_outcome_buy
test_multi_outcome_sell
test_max_outcomes_boundary
test_min_collateral_enforced
test_max_collateral_enforced
test_zero_amount_rejected
test_overflow_protection
test_slippage_calculation
test_quote_buy_matches_actual_buy
```

**Market Lifecycle (15 tests)**
```
test_create_market_basic
test_create_market_requires_reputation
test_create_market_fee_charged
test_duplicate_question_hash_rejected
test_market_opens_after_liquidity
test_market_closes_at_slot
test_closed_market_rejects_trades
test_market_categories
test_max_markets_enforced
test_min_max_duration
test_create_multi_outcome_market
test_emergency_pause_stops_trading
test_emergency_unpause_resumes
test_voided_market_refund
```

**Resolution (20 tests)**
```
test_submit_resolution_basic
test_resolution_requires_reputation
test_resolution_bond_posted
test_oracle_attestation_verified
test_insufficient_attestations_rejected
test_dispute_period_enforced
test_no_dispute_finalizes
test_challenge_posts_bond
test_challenge_escalates_to_disputed
test_dao_resolve_confirms
test_dao_resolve_overrides
test_dao_void_refunds_all
test_resolution_timeout_voids
test_resolver_reward_paid
test_loser_bond_distributed
test_cannot_resolve_active_market
test_cannot_resolve_twice
test_only_admin_dao_resolve
test_emergency_resolution
test_multiple_outcomes_resolution
```

**Settlement (15 tests)**
```
test_redeem_winning_shares
test_redeem_losing_shares_zero
test_partial_redemption
test_full_market_settlement
test_lp_withdrawal_on_resolution
test_collateral_fully_distributed
test_voided_market_pro_rata_refund
test_reclaim_all_outcomes
test_no_double_redemption
test_fees_distributed_correctly
test_protocol_fee_to_treasury
test_lp_fee_proportional
test_staker_fee_share
test_creator_gets_resolution_reward
test_rounding_dust_handled
```

**Integration (10 tests)**
```
test_full_market_lifecycle_binary
test_full_market_lifecycle_multi
test_multiple_traders_same_market
test_lp_profit_loss_scenarios
test_disputed_market_dao_resolution
test_agent_trading_loop
test_oracle_price_feed_resolution
test_category_browsing
test_user_portfolio_tracking
test_concurrent_markets
```

---

## 20. Open Questions

### Design Decisions for Discussion

| Question | Options | Recommendation |
|----------|---------|----------------|
| Should markets be permissionless or curated? | Permissionless (reputation-gated), Curated (admin-approved), Both | **Reputation-gated** — 500 rep creates, admin can delist |
| CLOB for prediction markets? | Phase 1 (integrated), Phase 2 (later), Never | **Phase 2** — AMM-only first, CLOB for deep markets later |
| Multi-outcome limit? | 2-only, Up to 8, Up to 16 | **Up to 8** — covers most use cases, keeps math tractable |
| Resolution oracle model? | Pure MoltOracle, Specialized oracle set, UMA-style optimistic | **MoltOracle attestation** — already deployed, multi-sig threshold |
| Share representation? | Storage balances, Separate tokens (MT-20), NFTs | **Storage balances** — simplest, cheapest, no cross-contract needed |
| Initial liquidity requirement? | Creator pays all, Platform subsidizes, Community pools | **Creator pays** — skin in the game, deducted from creation fee |

### Deferred Features

| Feature | Reason to Defer |
|---------|----------------|
| CLOB integration for outcome shares | Adds complexity, AMM sufficient for launch |
| Conditional markets (A if B) | Requires chained resolution, complex |
| Scalar markets (range outcomes) | Different AMM math, separate design |
| Leverage on predictions | Requires margin contract integration |
| Market maker incentive program | Needs usage data first |
| Automated resolution bots | Can be built by agents post-launch |

---

*End of PredictionReef Plan*
