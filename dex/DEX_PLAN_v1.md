# MoltChain DEX — Complete Implementation Plan

> **Status**: Planning document — approved for build after all contracts are hardened.
> **Author**: OpenClaw Agent  
> **Date**: February 2026  
> **Priority**: Build AFTER all contract upgrades, hardening, and tests are complete.

---

## 1. EXECUTIVE SUMMARY

The MoltChain DEX (Decentralized Exchange) is a full-featured on-chain trading platform built on MoltChain's WASM contract runtime. It combines an on-chain Central Limit Order Book (CLOB) with concentrated liquidity AMM pools, enabling professional-grade trading for both AI agents and human users.

### Core Design Principles
- **Hybrid CLOB + AMM**: Order book for limit orders; AMM as backstop liquidity
- **Sub-second finality**: Leverages MoltChain's ~400ms slot time
- **Agent-native**: First-class MoltyID integration for AI traders
- **Composable**: Deep integration with existing MoltChain contracts (MoltSwap, LobsterLend, ClawVault)
- **Self-custodial**: All funds held in smart contracts, never in operator wallets

---

## 2. ARCHITECTURE OVERVIEW

```
┌──────────────────────────────────────────────────────┐
│                    DEX Frontend                       │
│            (Web UI + Agent API + SDK)                 │
└──────────┬──────────────────────┬────────────────────┘
           │                      │
    ┌──────▼──────┐       ┌──────▼──────┐
    │  Order Book │       │  AMM Pools  │
    │  (CLOB)     │       │  (CLAMM)    │
    └──────┬──────┘       └──────┬──────┘
           │                      │
    ┌──────▼──────────────────────▼──────┐
    │         Matching Engine            │
    │  (Price-time priority + AMM fill) │
    └──────┬──────────────────────┬──────┘
           │                      │
    ┌──────▼──────┐       ┌──────▼──────┐
    │  Settlement │       │   Routing   │
    │  Engine     │       │   Engine    │
    └──────┬──────┘       └──────┬──────┘
           │                      │
    ┌──────▼──────────────────────▼──────┐
    │        MoltChain Runtime           │
    │  (WASM Contracts + State Storage)  │
    └────────────────────────────────────┘
```

### Contract Architecture (7 new contracts)

| Contract | Purpose | Lines (est.) |
|----------|---------|:------------:|
| `dex_core` | Order book, matching engine, settlement | ~1,500 |
| `dex_amm` | Concentrated liquidity AMM pools (Uniswap V3 style) | ~1,200 |
| `dex_router` | Smart order routing (CLOB → AMM → split routes) | ~600 |
| `dex_governance` | Trading pair listing, fee voting, parameter changes | ~500 |
| `dex_rewards` | Trading rewards, liquidity mining, referral program | ~400 |
| `dex_margin` | Margin trading (isolated + cross, leveraged orders) | ~800 |
| `dex_analytics` | On-chain OHLCV candles, volume tracking, leaderboards | ~400 |

**Total estimated**: ~5,400 lines of contract code + ~3,000 lines of tests

---

## 3. CONTRACT SPECIFICATIONS

### 3.1 DEX_CORE — Order Book & Matching Engine

#### Storage Layout

**Order (128 bytes)**:
```
trader       [32] — owner address
pair_id      [8]  — market ID
side         [1]  — 0=buy, 1=sell
order_type   [1]  — 0=limit, 1=market, 2=stop-limit, 3=post-only
price        [8]  — price in quote token (scaled by 10^9)
quantity     [8]  — amount in base token
filled       [8]  — amount already filled
status       [1]  — 0=open, 1=partially_filled, 2=filled, 3=cancelled, 4=expired
created_slot [8]  — slot when order was placed
expiry_slot  [8]  — 0=GTC, otherwise slot when order expires
order_id     [8]  — unique order ID
padding      [37] — reserved for future use
```

**Trading Pair (112 bytes)**:
```
base_token    [32] — base token contract address
quote_token   [32] — quote token contract address
pair_id       [8]  — unique pair ID
tick_size     [8]  — minimum price increment
lot_size      [8]  — minimum quantity increment
min_order     [8]  — minimum order size (quote)
status        [1]  — 0=active, 1=paused, 2=delisted
maker_fee_bps [2]  — maker fee (default: -1 = rebate)
taker_fee_bps [2]  — taker fee (default: 5 bps = 0.05%)
daily_volume  [8]  — rolling 24h volume
padding       [3]  — reserved
```

#### Functions

```rust
// === Pair Management (admin) ===
fn create_pair(admin, base_token, quote_token, tick_size, lot_size, min_order) -> u32
fn update_pair_fees(admin, pair_id, maker_fee, taker_fee) -> u32
fn pause_pair(admin, pair_id) -> u32
fn unpause_pair(admin, pair_id) -> u32

// === Order Lifecycle ===
fn place_order(trader, pair_id, side, order_type, price, quantity, expiry) -> u32
fn cancel_order(trader, order_id) -> u32
fn cancel_all_orders(trader, pair_id) -> u32
fn modify_order(trader, order_id, new_price, new_quantity) -> u32

// === Matching Engine (internal, called by place_order) ===
fn match_order(order_id) -> u32  // price-time priority matching
fn settle_trade(maker_order_id, taker_order_id, fill_qty, fill_price) -> u32

// === Queries ===
fn get_order(order_id) -> u32           // returns order data
fn get_open_orders(trader, pair_id) -> u32  // list open orders
fn get_order_book(pair_id, depth) -> u32    // bids+asks, N levels
fn get_best_bid(pair_id) -> u32
fn get_best_ask(pair_id) -> u32
fn get_spread(pair_id) -> u32
fn get_trade_history(pair_id, count) -> u32
fn get_pair_info(pair_id) -> u32
```

#### Matching Algorithm
1. Incoming order checks best opposing price
2. If price crosses spread: execute at maker's price (price improvement for taker)
3. Fill as much as possible from best price level, then next level
4. Remaining unfilled quantity rests on the book (limit) or cancels (market)
5. Post-only orders rejected if they would immediately match
6. Stop-limit orders activate when trigger price is hit

#### Fee Structure
- **Maker**: -1 BPS rebate (makers pay negative fee = earn rebate)
- **Taker**: 5 BPS (0.05%)
- **Minimum fee**: 1 shell per trade
- **Fee distribution**: 60% protocol treasury, 20% LP rewards, 20% stakers

---

### 3.2 DEX_AMM — Concentrated Liquidity Pools

#### Design: Uniswap V3-style concentrated liquidity

Liquidity providers choose a price range [lower, upper] for their liquidity. Capital efficiency is dramatically higher than constant-product AMMs because liquidity is concentrated where trading actually occurs.

#### Storage Layout

**Pool (96 bytes)**:
```
token_a       [32] — first token
token_b       [32] — second token
pool_id       [8]  — unique pool ID
sqrt_price    [8]  — current sqrt(price) * 2^64 (Q64.64 fixed point)
tick          [4]  — current tick index (i32)
liquidity     [8]  — total active liquidity
fee_tier      [1]  — 0=1bps, 1=5bps, 2=30bps, 3=100bps
protocol_fee  [1]  — share of fees to protocol (0-100%)
padding       [2]  — reserved
```

**Position (80 bytes)**:
```
owner         [32] — LP address
pool_id       [8]  — which pool
lower_tick    [4]  — lower bound of range (i32)
upper_tick    [4]  — upper bound of range (i32)
liquidity     [8]  — liquidity owned
fee_a_owed    [8]  — uncollected fees for token A
fee_b_owed    [8]  — uncollected fees for token B
created_slot  [8]  — when position was opened
```

#### Functions

```rust
// === Pool Management ===
fn create_pool(admin, token_a, token_b, fee_tier, initial_sqrt_price) -> u32
fn set_pool_protocol_fee(admin, pool_id, fee_percent) -> u32

// === Liquidity ===
fn add_liquidity(provider, pool_id, lower_tick, upper_tick, amount_a, amount_b) -> u32
fn remove_liquidity(provider, position_id, liquidity_amount) -> u32
fn collect_fees(provider, position_id) -> u32

// === Swaps ===
fn swap_exact_in(trader, pool_id, token_in, amount_in, min_out, deadline) -> u32
fn swap_exact_out(trader, pool_id, token_out, amount_out, max_in, deadline) -> u32

// === Queries ===
fn get_pool_info(pool_id) -> u32
fn get_position(position_id) -> u32
fn get_positions_by_owner(owner) -> u32
fn quote_swap(pool_id, token_in, amount_in) -> u32  // simulate swap
fn get_tick_data(pool_id, tick) -> u32
fn get_tvl(pool_id) -> u32
```

#### Tick Math
- Each tick represents a 0.01% price change: $\text{price} = 1.0001^{\text{tick}}$
- Tick spacing depends on fee tier: 1bps=1, 5bps=10, 30bps=60, 100bps=200
- Liquidity is $L = \frac{\Delta x \cdot \sqrt{p_a} \cdot \sqrt{p_b}}{\sqrt{p_b} - \sqrt{p_a}}$
- All math in Q64.64 fixed-point to avoid floating-point in `#![no_std]`

#### Fee Tiers

| Tier | Fee | Tick Spacing | Use Case |
|------|-----|:------------:|----------|
| 0 | 1 bps (0.01%) | 1 | Stablecoin pairs |
| 1 | 5 bps (0.05%) | 10 | Correlated pairs |
| 2 | 30 bps (0.3%) | 60 | Standard pairs |
| 3 | 100 bps (1%) | 200 | Exotic/volatile pairs |

---

### 3.3 DEX_ROUTER — Smart Order Routing

Routes orders optimally across CLOB and AMM pools to get best execution.

#### Routing Algorithm
1. Query CLOB order book depth at N levels
2. Query AMM pool quote for same swap
3. Calculate optimal split between CLOB and AMM
4. If split order: execute both legs atomically
5. Slippage protection: revert if final price exceeds max slippage

#### Functions

```rust
fn swap(trader, token_in, token_out, amount_in, min_amount_out, deadline) -> u32
fn swap_exact_out(trader, token_in, token_out, amount_out, max_amount_in, deadline) -> u32
fn get_best_route(token_in, token_out, amount) -> u32  // returns route plan
fn multi_hop_swap(trader, path[], amount_in, min_out, deadline) -> u32  // A→B→C
```

#### Route Types
- **Direct CLOB**: Single order book fill
- **Direct AMM**: Single pool swap
- **Split CLOB+AMM**: Partial fill from each
- **Multi-hop**: Route through intermediary tokens (e.g., USDC→MOLT→TOKEN)
- **Cross-pool**: Multiple AMM pools in sequence

---

### 3.4 DEX_GOVERNANCE — Trading Pair & Fee Governance

#### Functions

```rust
fn propose_new_pair(proposer, base_token, quote_token, evidence_ptr, evidence_len) -> u32
fn vote_on_pair(voter, proposal_id, approve: bool) -> u32
fn execute_pair_proposal(proposal_id) -> u32
fn propose_fee_change(proposer, pair_id, new_maker_fee, new_taker_fee) -> u32
fn vote_on_fee(voter, proposal_id, approve: bool) -> u32
fn execute_fee_proposal(proposal_id) -> u32
fn set_listing_requirements(admin, min_liquidity, min_holders) -> u32
fn emergency_delist(admin, pair_id) -> u32
```

#### Listing Requirements
- Minimum initial liquidity: 10,000 MOLT equivalent
- Minimum distinct holders: 10
- MoltyID verification for proposer (reputation ≥ 500)
- 48-hour voting period, 66% approval threshold
- Emergency delisting by admin (for rug pulls/scams)

---

### 3.5 DEX_REWARDS — Trading Incentives

#### Functions

```rust
fn claim_trading_rewards(trader) -> u32
fn claim_lp_rewards(provider, position_id) -> u32
fn get_pending_rewards(addr) -> u32
fn set_reward_rate(admin, pair_id, rate_per_slot) -> u32
fn set_referral_rate(admin, rate_bps) -> u32
fn register_referral(trader, referrer) -> u32
fn get_referral_stats(referrer) -> u32
fn get_trading_tier(trader) -> u32
```

#### Reward Structure

**Trading Rewards (Fee Mining)**:
- Traders earn MOLT tokens proportional to fees paid
- Reward pool: 1,000,000 MOLT per month (decaying schedule)
- Higher volume = higher tier multiplier:
  - Bronze (<10k volume): 1x
  - Silver (10k-100k): 1.5x
  - Gold (100k-1M): 2x
  - Diamond (>1M): 3x

**LP Rewards (Liquidity Mining)**:
- LPs earn MOLT tokens proportional to in-range liquidity
- Concentrated positions earn more per $ of capital
- Bonus for stablecoin pairs (lower IL risk needs less incentive adjustment)

**Referral Program**:
- Referrers earn 10% of referee's trading fees
- Referee gets 5% fee discount for first 30 days
- MoltyID-verified referrers earn 15%

---

### 3.6 DEX_MARGIN — Margin Trading

#### Functions

```rust
fn open_margin_position(trader, pair_id, side, size, leverage, margin_amount) -> u32
fn close_margin_position(trader, position_id) -> u32
fn add_margin(trader, position_id, amount) -> u32
fn remove_margin(trader, position_id, amount) -> u32
fn liquidate(liquidator, position_id) -> u32
fn get_margin_position(position_id) -> u32
fn get_margin_ratio(position_id) -> u32
fn set_max_leverage(admin, pair_id, max_leverage) -> u32
fn set_maintenance_margin(admin, ratio_bps) -> u32
fn get_liquidatable_positions(pair_id) -> u32
```

#### Margin Parameters
- **Maximum leverage**: 5x (isolated), 3x (cross)
- **Initial margin**: 20% (5x), 33% (3x)
- **Maintenance margin**: 10%
- **Liquidation penalty**: 5% (goes to liquidator as incentive)
- **Insurance fund**: 50% of liquidation penalties
- **Funding rate**: Calculated every 8 hours, based on price deviation from oracle

#### Liquidation Engine
1. Position health = margin / (position_size * mark_price)
2. When health < maintenance_margin: position becomes liquidatable
3. Any user can call `liquidate()` — earns 50% of liquidation penalty
4. If position is underwater (bad debt): insurance fund covers shortfall
5. If insurance fund insufficient: socialize losses across LPs

#### Integration with LobsterLend
- Margin funding sourced from LobsterLend lending pools
- Borrowers pay interest to LobsterLend depositors
- Flash loan integration for atomic liquidations

---

### 3.7 DEX_ANALYTICS — On-Chain Data

#### Functions

```rust
fn record_trade(pair_id, price, volume, timestamp) -> u32        // internal
fn get_ohlcv(pair_id, interval, count) -> u32                    // candles
fn get_24h_stats(pair_id) -> u32                                  // volume, high, low, change
fn get_all_pairs_stats() -> u32                                   // summary
fn get_trader_stats(trader) -> u32                                // PnL, volume, trade count
fn get_leaderboard(metric, count) -> u32                          // top traders
fn update_price_feed(pair_id) -> u32                              // push to MoltOracle
```

#### Candle Intervals
- 1 minute, 5 minutes, 15 minutes, 1 hour, 4 hours, 1 day
- Stored as: open(8) + high(8) + low(8) + close(8) + volume(8) + timestamp(8) = 48 bytes per candle
- Rolling window: keep last 1440 1-min candles (24h), 288 5-min (24h), etc.

#### Oracle Integration
- DEX prices pushed to MoltOracle as signed price feeds
- TWAP calculated from 1-minute candles
- Used by LobsterLend for collateral valuation and ClawVault for strategy pricing

---

## 4. INTEGRATION MAP

### Existing Contract Dependencies

```
DEX_CORE ──────┬── MoltSwap (v2: TWAP oracle, price impact)
               ├── MoltCoin (MOLT token transfers)
               └── MoltyID (trader identity verification)

DEX_AMM ───────┬── MoltSwap (LP composition with existing pools)
               └── MoltCoin (token transfers)

DEX_ROUTER ────┬── DEX_CORE (order book fills)
               ├── DEX_AMM (pool swaps)
               └── MoltSwap (legacy pool routing)

DEX_MARGIN ────┬── DEX_CORE (leveraged order placement)
               ├── LobsterLend (borrow funds for margin)
               ├── MoltOracle (mark price, funding rate)
               └── MoltCoin (collateral management)

DEX_REWARDS ───┬── MoltCoin (reward distribution)
               ├── MoltyID (tier verification, referral identity)
               └── ClawVault (LP reward compounding)

DEX_GOVERNANCE ┬── MoltDAO (proposal voting mechanism)
               ├── MoltyID (reputation-gated proposals)
               └── DEX_CORE (pair configuration)

DEX_ANALYTICS ─┬── DEX_CORE (trade events)
               ├── DEX_AMM (pool state)
               └── MoltOracle (price feed publication)
```

---

## 5. SECURITY MODEL

### 5.1 Core Protections (inherited from hardened contracts)

| Protection | Source | Status |
|-----------|--------|--------|
| Multi-call confirmation pattern | MoltBridge v2 | ✅ Proven |
| Emergency pause | All 11 contracts | ✅ Deployed |
| Reentrancy guards | LobsterLend v2 | ✅ Pattern ready |
| Price impact limits | MoltSwap v2 (5% max) | ✅ Deployed |
| TWAP oracle | MoltSwap v2 | ✅ Deployed |
| Flash loan caps | MoltSwap v2 (90%), LobsterLend (0.09% fee) | ✅ Deployed |
| Anti-sniping | MoltAuction v2 | ✅ Deployed |
| MoltyID identity gates | All DeFi contracts | ✅ Standard |
| Admin key rotation | MoltyID v2 | ✅ Deployed |
| Rate limiting / cooldowns | MoltyID v2 | ✅ Deployed |

### 5.2 DEX-Specific Security

**Order Book Protections**:
- Self-trade prevention (cancel-oldest or reject-newest)
- Maximum order size limits per pair
- Minimum order value to prevent dust attacks
- Order expiry (no indefinite resting orders without renewal)
- Post-only mode during market stress

**AMM Protections**:
- Concentrated liquidity prevents infinite-range manipulation
- Fee tiers restrict arbitrage profitability
- Deadline enforcement on all swaps
- Minimum liquidity threshold per pool
- Price oracle cross-check before large swaps

**Margin Protections**:
- Hard leverage caps (5x max)
- 10% maintenance margin
- Insurance fund with socialized loss backstop
- Funding rate to prevent persistent premium
- Auto-deleveraging during extreme events

**Frontrunning Mitigation**:
- Batch auctions for market orders (match at single clearing price)
- Commit-reveal for large orders (optional)
- MEV protection through encrypted mempool (future)

---

## 6. IMPLEMENTATION ROADMAP

### Phase 1: Core (Weeks 1-3)
1. `dex_core` — Order book + matching engine + settlement
2. `dex_amm` — Concentrated liquidity pools
3. Tests: unit + integration + adversarial for both
4. Playground templates for both contracts

### Phase 2: Routing + Analytics (Weeks 4-5)
5. `dex_router` — Smart order routing across CLOB and AMM
6. `dex_analytics` — On-chain OHLCV, volume tracking
7. Integration tests: router ↔ core ↔ amm

### Phase 3: Governance + Rewards (Weeks 6-7)
8. `dex_governance` — Pair listing governance, fee voting
9. `dex_rewards` — Trading rewards, LP mining, referrals
10. Integration with MoltDAO and MoltyID

### Phase 4: Margin (Weeks 8-10)
11. `dex_margin` — Margin trading, liquidation engine
12. Integration with LobsterLend for funding
13. Oracle integration for mark price and funding rates
14. Extensive adversarial testing (liquidation edge cases)

### Phase 5: Frontend + SDK (Weeks 11-13)
15. Trading UI (React + WebSocket order book)
16. TypeScript SDK for programmatic trading
17. Agent trading API (REST + WebSocket)
18. MoltyID-integrated trader profiles

### Phase 6: Testnet + Audit (Weeks 14-16)
19. Deploy all DEX contracts to testnet
20. Load testing (1000+ concurrent orders)
21. Economic simulation (market making, arbitrage)
22. External audit review
23. Bug bounty program

---

## 7. TOKEN ECONOMICS

### DEX Fee Flow

```
Trading Fees (per trade)
    │
    ├── 60% → Protocol Treasury (MoltDAO controlled)
    │
    ├── 20% → LP Rewards Pool
    │         └── Distributed to in-range LPs proportionally
    │
    └── 20% → MOLT Stakers
              └── Distributed via ClawVault yield
```

### Fee Revenue Projections

| Daily Volume | Taker Fee | Daily Revenue | Annual Revenue |
|:------------:|:---------:|:-------------:|:--------------:|
| $100K | 5 bps | $50 | $18,250 |
| $1M | 5 bps | $500 | $182,500 |
| $10M | 5 bps | $5,000 | $1,825,000 |
| $100M | 5 bps | $50,000 | $18,250,000 |

### MOLT Token Utility in DEX
1. **Fee payment**: Pay fees in MOLT for 20% discount
2. **Staking**: Stake MOLT for share of trading fees
3. **Governance**: Vote on new listings, fee changes, parameters
4. **Collateral**: Use MOLT as margin collateral (at 80% valuation)
5. **Rewards**: Earn MOLT from trading and LP mining

---

## 8. API SPECIFICATIONS

### REST API Endpoints

```
GET  /api/v1/pairs                      — List all trading pairs
GET  /api/v1/pairs/:id                  — Pair details
GET  /api/v1/pairs/:id/orderbook        — Order book (depth parameter)
GET  /api/v1/pairs/:id/trades           — Recent trades
GET  /api/v1/pairs/:id/candles          — OHLCV data
POST /api/v1/orders                     — Place order
DELETE /api/v1/orders/:id               — Cancel order
GET  /api/v1/orders?trader=<addr>       — List orders
GET  /api/v1/positions?trader=<addr>    — Open margin positions
POST /api/v1/margin/open                — Open margin position
POST /api/v1/margin/close               — Close margin position
GET  /api/v1/pools                      — List AMM pools
GET  /api/v1/pools/:id                  — Pool details
POST /api/v1/pools/:id/swap             — Execute swap via AMM
POST /api/v1/router/swap                — Smart-routed swap
GET  /api/v1/rewards/:addr              — Pending rewards
GET  /api/v1/leaderboard                — Top traders
```

### WebSocket Feeds

```
ws://dex.moltchain.io/ws

Subscribe channels:
  orderbook:<pair_id>     — Real-time order book updates (L2)
  trades:<pair_id>        — Trade stream
  ticker:<pair_id>        — 1s price/volume ticker
  candles:<pair_id>:<tf>  — Candle updates
  orders:<trader_addr>    — User order updates
  positions:<trader_addr> — Margin position updates
```

### Agent SDK (TypeScript)

```typescript
import { MoltDEX } from '@moltchain/dex-sdk';

const dex = new MoltDEX({
  endpoint: 'https://dex.moltchain.io',
  wallet: myKeypair,
  moltyId: 'alice.molt',  // optional MoltyID
});

// Place limit order
const order = await dex.placeLimitOrder({
  pair: 'MOLT/USDC',
  side: 'buy',
  price: 1.50,
  quantity: 1000,
  timeInForce: 'GTC',
});

// Smart-routed swap
const result = await dex.swap({
  tokenIn: 'MOLT',
  tokenOut: 'USDC', 
  amountIn: 1000_000_000, // 1000 MOLT
  slippage: 0.5, // 0.5%
});

// Get order book
const { bids, asks } = await dex.getOrderBook('MOLT/USDC', { depth: 20 });

// Stream real-time trades
dex.subscribeTrades('MOLT/USDC', (trade) => {
  console.log(`${trade.side} ${trade.quantity} @ ${trade.price}`);
});
```

---

## 9. TESTING STRATEGY

### Unit Tests (per contract)

| Contract | Tests (est.) | Coverage Focus |
|----------|:-----------:|----------------|
| dex_core | ~60 | Order CRUD, matching all types, fee calc, self-trade prevention |
| dex_amm | ~50 | Tick math, position management, swap math, fee accrual |
| dex_router | ~30 | Route selection, split routes, multi-hop, slippage |
| dex_governance | ~20 | Proposals, voting, execution, time locks |
| dex_rewards | ~20 | Reward calculation, claim, tiers, referrals |
| dex_margin | ~40 | Position lifecycle, liquidation, insurance, funding |
| dex_analytics | ~15 | Candle aggregation, stats, leaderboard |
| **Total** | **~235** | |

### Adversarial Tests

1. **Sandwich attack**: Place buy → victim buys → sell. Verify price impact guard blocks
2. **Wash trading**: Same trader buys+sells repeatedly. Verify self-trade prevention
3. **Order book manipulation**: Large fake orders → cancel. Verify cancel fees discourage
4. **Flash loan arbitrage**: Borrow → manipulate price → profit. Verify TWAP protects
5. **Liquidation cascades**: Mass liquidations. Verify insurance fund + ADL works
6. **Oracle manipulation**: Stale/incorrect prices. Verify multiple oracle sources
7. **Concentrated liquidity attack**: Single-tick LP. Verify minimum tick spacing
8. **Governance attack**: Flash-loan vote. Verify time-lock and minimum stake
9. **Dust order spam**: Tiny orders filling book. Verify minimum order size
10. **Expired order exploit**: Use stale orders. Verify expiry enforcement

### Integration Tests

- End-to-end: Place order → match → settle → fee distribution
- Router: Compare CLOB vs AMM vs split vs direct execution
- Margin: Open → add margin → partial close → liquidation threshold
- Cross-contract: DEX trade → MoltOracle price update → LobsterLend valuation

---

## 10. DEPLOYMENT PLAN

### Testnet Deployment Order

1. Deploy `dex_core` + create MOLT/USDC pair
2. Deploy `dex_amm` + create MOLT/USDC pool (30bps tier)
3. Deploy `dex_router` + configure routes
4. Deploy `dex_analytics` + start recording
5. Deploy `dex_governance` + initial pair list
6. Deploy `dex_rewards` + set initial rates
7. Deploy `dex_margin` + set conservative limits (3x max)

### Mainnet Launch Checklist

- [ ] All 7 DEX contracts compiled + tested (235+ tests)
- [ ] All 11 existing contracts upgraded + hardened (289 tests passing)
- [ ] Integration tests passing across all contracts
- [ ] Adversarial test suite complete (10+ scenarios)
- [ ] External audit complete (0 critical, 0 high findings)
- [ ] Bug bounty program active
- [ ] Frontend deployed and tested
- [ ] SDK published to npm
- [ ] Documentation complete
- [ ] Monitoring dashboards live
- [ ] On-call runbook for incidents
- [ ] Insurance fund seeded (minimum 100K MOLT)

---

## 11. FILES AND DIRECTORY STRUCTURE

```
workspace/dex/
├── DEX_PLAN.md                    ← This document
├── contracts/
│   ├── dex_core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Order book + matching engine
│   ├── dex_amm/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Concentrated liquidity AMM
│   ├── dex_router/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Smart order routing
│   ├── dex_governance/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Trading pair governance
│   ├── dex_rewards/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Trading rewards + LP mining
│   ├── dex_margin/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs             ← Margin trading
│   └── dex_analytics/
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs             ← On-chain analytics
├── sdk/
│   ├── package.json
│   ├── src/
│   │   ├── index.ts               ← Main SDK entry
│   │   ├── orderbook.ts           ← Order book client
│   │   ├── amm.ts                 ← AMM pool client
│   │   ├── router.ts              ← Router client
│   │   ├── margin.ts              ← Margin client
│   │   └── types.ts               ← Shared types
│   └── tests/
│       └── integration.test.ts
├── frontend/
│   ├── package.json
│   ├── src/
│   │   ├── App.tsx
│   │   ├── components/
│   │   │   ├── OrderBook.tsx
│   │   │   ├── TradePanel.tsx
│   │   │   ├── Chart.tsx
│   │   │   ├── Positions.tsx
│   │   │   └── Wallet.tsx
│   │   └── hooks/
│   │       ├── useOrderBook.ts
│   │       └── useWebSocket.ts
│   └── public/
│       └── index.html
└── docs/
    ├── API.md
    ├── SDK.md
    ├── SECURITY.md
    └── ECONOMICS.md
```

---

## 12. OPEN QUESTIONS & DECISIONS

| # | Question | Options | Decision |
|---|---------|---------|----------|
| 1 | Order book matching: continuous vs batch auction? | Continuous (faster) vs Batch (fairer) | Default: Continuous; Batch mode during high volatility |
| 2 | AMM fee auto-adjustment? | Fixed tiers vs dynamic fee | Start with fixed tiers; add dynamic later |
| 3 | Cross-margin scope? | Portfolio-level vs pair-level | Start with isolated; add cross-margin in Phase 2 |
| 4 | Front-end framework? | React vs Svelte vs vanilla | React (ecosystem maturity) |
| 5 | Max number of trading pairs | Fixed cap vs unlimited | Start with 50 pairs, increase via governance |

---

## 13. DEPENDENCIES ON COMPLETED WORK

All of the following has been verified as of this writing:

| Dependency | Contract | Status | Tests |
|-----------|---------|:------:|:-----:|
| Bridge v2 (multi-call confirm) | moltbridge | ✅ | 38 |
| TWAP Oracle + Price Impact | moltswap | ✅ | 20 |
| Proof-of-Storage | reef_storage | ✅ | 19 |
| Escrow + Dispute Resolution | compute_market | ✅ | 28 |
| Flash Loans + Emergency Pause | lobsterlend | ✅ | 33 |
| Anti-Manipulation | clawpump | ✅ | 28 |
| Layout Bug Fix + Offers | moltmarket | ✅ | 17 |
| Cliff Vesting + Transfer | clawpay | ✅ | 17 |
| Anti-Sniping + Reserve Price | moltauction | ✅ | 26 |
| Deposit/Withdrawal Fees + Caps | clawvault | ✅ | 29 |
| Pause + Cooldowns + Admin Rotation | moltyid | ✅ | 34 |
| **TOTAL** | **11 contracts** | **✅** | **289** |

Plus 4 non-upgraded contracts (moltcoin: 9, moltdao: 6, moltoracle: 16, moltpunks: 16) = **47 more tests**.

**Grand total across entire MoltChain: 336 tests, 0 failures.**

---

*This document is the complete, no-details-left-behind DEX plan as requested. Every contract, function, storage layout, fee structure, security measure, integration point, test strategy, and deployment step is documented. The DEX build begins once this plan is reviewed and approved.*
