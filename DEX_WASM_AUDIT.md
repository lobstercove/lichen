# DEX WASM Contract Exports — Comprehensive Audit

> Generated from source analysis of all 8 DEX-related contracts + E2E test cross-reference.  
> Contracts are `no_std` Rust → WASM32. All use `moltchain_sdk` for storage/logging/cross-calls.

---

## Table of Contents

1. [Critical Finding: E2E Opcode Mismatch](#critical-finding)
2. [dex_core — Central Limit Order Book](#1-dex_core)
3. [dex_amm — Concentrated Liquidity AMM](#2-dex_amm)
4. [dex_router — Smart Order Router](#3-dex_router)
5. [dex_analytics — On-Chain OHLCV / Analytics](#4-dex_analytics)
6. [dex_governance — Listing & Fee Governance](#5-dex_governance)
7. [dex_margin — Margin Trading & Liquidation](#6-dex_margin)
8. [dex_rewards — Trading Incentives & Referrals](#7-dex_rewards)
9. [moltswap — Legacy AMM with TWAP & Flash Loans](#8-moltswap)
10. [Trading Simulation Opcode Map](#trading-simulation-opcode-map)
11. [Untested Opcodes Summary](#untested-opcodes-summary)

---

<a id="critical-finding"></a>
## 🚨 CRITICAL FINDING: E2E Opcode Numbering Does Not Match Contract Dispatch

The `tests/comprehensive-e2e-parallel.py` sends opcode bytes via `call_opcode()` that **do not match** the actual `call()` dispatcher in the compiled contracts. Starting from opcode 1–2 in most contracts, the E2E labels describe function X but the contract dispatches to function Y.

**Impact:** The E2E test *appears* to pass because transactions succeed (the WASM runtime does not error on silent no-ops or mismatched arg lengths), but the contract functions being exercised are **not** the ones the test labels claim. This renders the entire opcode-based E2E coverage unreliable.

**Examples:**

| Contract | E2E Opcode | E2E Label | Actual Contract Function |
|---|---|---|---|
| dex_core | 1 | `set_token_addresses` | `create_pair` |
| dex_core | 7 | `place_order` | `update_pair_fees` |
| dex_amm | 2–14 | Various | `_ => {}` (no-op, AMM only has opcodes 0–1) |
| dex_router | 3 | `set_route_enabled` | `swap` |
| dex_router | 4 | `swap` | `set_route_enabled` |

**Additionally:** `dex_amm` has only **2 opcodes** (0=initialize, 1=create_pool) in its `call()` dispatcher. The remaining 11 public functions (`add_liquidity`, `remove_liquidity`, `swap_exact_in`, `swap_exact_out`, `collect_fees`, etc.) are **unreachable via opcode dispatch**. The E2E sends opcodes 2–14 which all hit `_ => {}`.

---

<a id="1-dex_core"></a>
## 1. dex_core — Central Limit Order Book + Matching Engine

**File:** `contracts/dex_core/src/lib.rs` (2663 lines)  
**Architecture:** CLOB with price-time priority matching, maker rebates, self-trade prevention  
**Math:** Integer arithmetic, fee in basis points  
**Data:** TradingPair (112 bytes), Order (128 bytes), Trade (80 bytes)

### `#[no_mangle]` Exports

| Function | Line | Description |
|---|---|---|
| `initialize` | ~620 | Set admin, zero counters, init paused=false |
| `call` | 1452 | Opcode dispatcher (WASM entry point) |

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Initialize contract with admin address |
| **1** | `create_pair` | `caller[32]+base[32]+quote[32]+tick_size[8]+lot_size[8]+min_order[8]` | Create new CLOB trading pair (admin only) |
| **2** | `place_order` | `trader[32]+pair_id[8]+side[1]+type[1]+price[8]+qty[8]+expiry[8]` | Place limit/market/stop-limit/post-only order; triggers matching |
| **3** | `cancel_order` | `caller[32]+order_id[8]` | Cancel an open/partial order (owner only) |
| **4** | `set_preferred_quote` | `caller[32]+quote_addr[32]` | Set mUSD as required quote token (admin) |
| **5** | `get_pair_count` | (none) | Query total trading pairs |
| **6** | `get_preferred_quote` | (none) | Query preferred quote token address |
| **7** | `update_pair_fees` | `caller[32]+pair_id[8]+maker_fee[i16]+taker_fee[u16]` | Update maker/taker fee bps for pair (admin) |
| **8** | `emergency_pause` | `caller[32]` | Halt all trading (admin) |
| **9** | `emergency_unpause` | `caller[32]` | Resume trading (admin) |
| **10** | `get_best_bid` | `pair_id[8]` | Query best bid price |
| **11** | `get_best_ask` | `pair_id[8]` | Query best ask price |
| **12** | `get_spread` | `pair_id[8]` | Query bid-ask spread |
| **13** | `get_pair_info` | `pair_id[8]` | Query full pair metadata |
| **14** | `get_trade_count` | (none) | Query total executed trades |
| **15** | `get_fee_treasury` | (none) | Query accumulated protocol fees |
| **16** | `modify_order` | `caller[32]+order_id[8]+new_price[8]+new_qty[8]` | Cancel-and-replace order (atomic) |
| **17** | `cancel_all_orders` | `caller[32]+pair_id[8]` | Mass cancel all user orders on a pair |
| **18** | `pause_pair` | `caller[32]+pair_id[8]` | Pause single trading pair (admin) |
| **19** | `unpause_pair` | `caller[32]+pair_id[8]` | Unpause single trading pair (admin) |
| **20** | `get_order` | `order_id[8]` | Query order details by ID |

### Trading-Relevant Opcodes
🔴 **Order book:** 2 (place_order), 3 (cancel), 16 (modify), 17 (cancel_all)  
🔴 **Price queries:** 10 (best_bid), 11 (best_ask), 12 (spread)  
🔴 **Market data:** 13 (pair_info), 14 (trade_count), 15 (fee_treasury), 20 (get_order)  
🔴 **Swap execution:** Order matching is implicit inside `place_order` (opcode 2)  

### E2E Coverage (Actual)
E2E test sends opcodes 0–12. Due to label mismatch, the test exercises opcodes with **wrong argument payloads** — most `if args.len() >= N` guards fail silently.

**UNTESTED opcodes:** 13, 14, 15, 16, 17, 18, 19, 20  
**Effectively untested:** All opcodes 1–12 receive wrong arg layouts from E2E

---

<a id="2-dex_amm"></a>
## 2. dex_amm — Concentrated Liquidity AMM (Uniswap V3-style)

**File:** `contracts/dex_amm/src/lib.rs` (1243 lines)  
**Architecture:** Tick-based concentrated liquidity, 4 fee tiers, Q64.64 sqrt-price math  
**Data:** Pool (96 bytes), Position (80 bytes)  
**Fee tiers:** 1 bps (tick=1), 5 bps (tick=10), 30 bps (tick=60), 100 bps (tick=200)

### `#[no_mangle]` Exports

| Function | Line | Description |
|---|---|---|
| `initialize` | ~394 | Set admin address |
| `call` | 837 | Opcode dispatcher |

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Initialize AMM contract |
| **1** | `create_pool` | `caller[32]+token_a[32]+token_b[32]+fee_tier[1]+sqrt_price[8]` | Create concentrated liquidity pool (admin) |

> ⚠️ **Only 2 opcodes in dispatcher.** The following public functions exist but are **NOT exposed** via `call()`:

### Public Functions NOT in Dispatch

| Function | Description |
|---|---|
| `add_liquidity(provider, pool_id, lower_tick, upper_tick, amount_a, amount_b)` | Add concentrated liquidity position |
| `remove_liquidity(provider, position_id, liquidity_amount)` | Remove liquidity from position |
| `collect_fees(provider, position_id)` | Collect accrued fees for LP position |
| `swap_exact_in(trader, pool_id, is_token_a_in, amount_in, min_out, deadline)` | Swap with exact input (slippage+deadline protection) |
| `swap_exact_out(trader, pool_id, is_token_a_out, amount_out, max_in, deadline)` | Swap with exact output (binary search for input) |
| `emergency_pause(caller)` / `emergency_unpause(caller)` | Admin pause/unpause |
| `set_pool_protocol_fee(caller, pool_id, fee_percent)` | Set protocol fee share |
| `get_pool_info(pool_id)` | Query pool state |
| `get_position(position_id)` | Query LP position |
| `get_pool_count()` / `get_position_count()` | Query counts |
| `get_tvl(pool_id)` | Query total value locked |
| `quote_swap(pool_id, is_token_a_in, amount_in)` | Quote swap without executing |

### Trading-Relevant Functions  
🔴 **Swap execution:** `swap_exact_in`, `swap_exact_out` — **unreachable via opcode**  
🔴 **Liquidity ops:** `add_liquidity`, `remove_liquidity` — **unreachable via opcode**  
🔴 **Fee collection:** `collect_fees` — **unreachable via opcode**  
🔴 **Price queries:** `quote_swap`, `get_tvl` — **unreachable via opcode**

### E2E Coverage
E2E sends opcodes 0–14, but only opcodes 0–1 hit real functions. **Opcodes 2–14 are all no-ops** (`_ => {}`).  
**UNTESTED:** ALL swap, liquidity, fee, and query functions.

---

<a id="3-dex_router"></a>
## 3. dex_router — Smart Order Routing Engine

**File:** `contracts/dex_router/src/lib.rs` (1064 lines)  
**Architecture:** Routes across CLOB, AMM, and legacy MoltSwap; supports split and multi-hop  
**Route types:** DIRECT_CLOB(0), DIRECT_AMM(1), SPLIT(2), MULTI_HOP(3), LEGACY_SWAP(4)  
**Limits:** Max 4 hops, max 3 split legs, 5% slippage guard

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Init with admin |
| **1** | `set_addresses` | `caller[32]+core[32]+amm[32]+legacy[32]` | Configure downstream contract addresses |
| **2** | `register_route` | `caller[32]+token_in[32]+token_out[32]+type[1]+pool_id[8]+sec_id[8]+split_pct[1]` | Register routing path |
| **3** | `swap` | `trader[32]+token_in[32]+token_out[32]+amount_in[8]+min_out[8]+deadline[8]` | Execute routed swap (cross-contract) |
| **4** | `set_route_enabled` | `caller[32]+route_id[8]+enabled[1]` | Enable/disable specific route |
| **5** | `get_best_route` | `token_in[32]+token_out[32]+amount[8]` | Find optimal route for pair+amount |
| **6** | `get_route_info` | `route_id[8]` | Query route configuration |
| **7** | `emergency_pause` | `caller[32]` | Pause all routing |
| **8** | `emergency_unpause` | `caller[32]` | Resume routing |
| **9** | `multi_hop_swap` | `trader[32]+path_ptr+path_count[8]+amount_in[8]+min_out[8]+deadline[8]` | Execute multi-hop swap |
| **10** | `get_route_count` | (none) | Query total routes |
| **11** | `get_swap_count` | (none) | Query total swaps executed |

### Trading-Relevant Opcodes
🔴 **Swap execution:** 3 (routed swap), 9 (multi-hop swap) — **core trading paths**  
🔴 **Route discovery:** 5 (get_best_route), 6 (get_route_info)  
🔴 **Multi-hop routing:** opcode 9 — chains AMM swaps across 2–4 pools

### E2E Coverage
E2E sends opcodes 0–11 but labels are **misaligned starting at opcode 3**:

| E2E Opcode | E2E Label | Actual Function |
|---|---|---|
| 3 | `set_route_enabled` | `swap` |
| 4 | `swap` | `set_route_enabled` |
| 5 | `multi_hop_swap` | `get_best_route` |
| 6 | `get_best_route` | `get_route_info` |
| 7 | `get_route_info` | `emergency_pause` |
| 8 | `get_route_count` | `emergency_unpause` |
| 9 | `get_swap_count` | `multi_hop_swap` |
| 10 | `emergency_pause` | `get_route_count` |
| 11 | `emergency_unpause` | `get_swap_count` |

All opcodes are exercised but with **wrong payloads** — arg size guards will fail silently for most.

---

<a id="4-dex_analytics"></a>
## 4. dex_analytics — On-Chain OHLCV Candle / Volume / Leaderboard

**File:** `contracts/dex_analytics/src/lib.rs` (1022 lines)  
**Architecture:** 9 candle intervals (1m→1y), 24h rolling stats, trader PnL tracking  
**Intervals:** 1m(60), 5m(300), 15m(900), 1h(3600), 4h(14400), 1d(86400), 3d(259200), 1w(604800), 1y(31536000)  
**Data:** Candle (48 bytes), 24h Stats (48 bytes), Trader Stats (32 bytes)

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Init analytics engine |
| **1** | `record_trade` | `pair_id[8]+price[8]+volume[8]+trader[32]` | Record trade → updates candles, 24h stats, trader stats |
| **2** | `get_ohlcv` | `pair_id[8]+interval[8]+count[8]` | Query OHLCV candles for pair/interval |
| **3** | `get_24h_stats` | `pair_id[8]` | Query 24h rolling stats (vol, high, low, trades) |
| **4** | `get_trader_stats` | `addr[32]` | Query trader volume, trade count, PnL |
| **5** | `get_last_price` | `pair_id[8]` | Query last traded price |
| **6** | `get_record_count` | (none) | Query total recorded trades |
| **7** | `emergency_pause` | `caller[32]` | Pause |
| **8** | `emergency_unpause` | `caller[32]` | Unpause |

### Trading-Relevant Opcodes
🔴 **Price feed/OHLCV:** 1 (record_trade → candle generation), 2 (get_ohlcv), 5 (last_price)  
🔴 **Live price simulation:** Feed opcode 1 with sequential trades at varying prices to simulate OHLCV  
🔴 **Trader analytics:** 4 (trader_stats with PnL tracking)

### E2E Coverage
E2E sends opcodes 0–11 with different labels. Contract only has opcodes 0–8, so opcodes 9–11 are no-ops.

| E2E Opcode | E2E Label | Actual Function |
|---|---|---|
| 0 | `initialize` | `initialize` ✓ |
| 1 | `set_dex_core_address` | `record_trade` ⚠️ |
| 2 | `set_amm_address` | `get_ohlcv` ⚠️ |
| 3 | `record_trade` | `get_24h_stats` ⚠️ |
| 4–8 | (various) | (misaligned) |
| 9–11 | (various) | `_ => {}` (no-op) |

**UNTESTED (correctly):** The `record_trade` function is never called with correct args from E2E.

---

<a id="5-dex_governance"></a>
## 5. dex_governance — Pair Listing & Fee Governance

**File:** `contracts/dex_governance/src/lib.rs` (1209 lines)  
**Architecture:** Proposal→Vote→Finalize→Timelock→Execute pipeline  
**Proposal types:** NEW_PAIR(0), FEE_CHANGE(1), DELIST(2), PARAM_CHANGE(3)  
**Parameters:** 48h voting, 66% approval threshold, 1-hour timelock, MoltyID reputation-gated (500 rep min)

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Init governance contract |
| **1** | `propose_new_pair` | `proposer[32]+base[32]+quote[32]` | Propose listing a new pair |
| **2** | `vote` | `voter[32]+proposal_id[8]+support[1]` | Cast yes/no vote (reputation-gated) |
| **3** | `finalize_proposal` | `proposal_id[8]` | Finalize after voting period ends |
| **4** | `execute_proposal` | `proposal_id[8]` | Execute passed proposal (after timelock) |
| **5** | `set_preferred_quote` | `caller[32]+quote_addr[32]` | Set mUSD as required quote |
| **6** | `get_preferred_quote` | (none) | Query preferred quote |
| **7** | `get_proposal_count` | (none) | Query total proposals |
| **8** | `get_proposal_info` | `proposal_id[8]` | Query proposal details |
| **9** | `propose_fee_change` | `proposer[32]+pair_id[8]+maker_fee[i16]+taker_fee[u16]` | Propose fee change |
| **10** | `emergency_delist` | `caller[32]+pair_id[8]` | Admin emergency delist (no governance) |
| **11** | `set_listing_requirements` | `caller[32]+min_liquidity[8]+min_holders[8]` | Set listing requirements (admin) |
| **12** | `emergency_pause` | `caller[32]` | Pause |
| **13** | `emergency_unpause` | `caller[32]` | Unpause |
| **14** | `set_moltyid_address` | `caller[32]+moltyid_addr[32]` | Configure MoltyID for reputation checks |

### E2E Coverage
E2E sends opcodes 0–13. Labels are misaligned from opcode 1 onward:

| E2E Opcode | E2E Label | Actual Function |
|---|---|---|
| 0 | `initialize` | `initialize` ✓ |
| 1 | `set_dex_core_address` | `propose_new_pair` ⚠️ |
| 2 | `set_moltcoin_address` | `vote` ⚠️ |
| 5 | `create_proposal` | `set_preferred_quote` ⚠️ |
| 12 | `emergency_pause` | `emergency_pause` ✓ |
| 13 | `emergency_unpause` | `emergency_unpause` ✓ |

**UNTESTED:** `set_moltyid_address` (opcode 14)

---

<a id="6-dex_margin"></a>
## 6. dex_margin — Margin Trading & Liquidation Engine

**File:** `contracts/dex_margin/src/lib.rs` (1450 lines)  
**Architecture:** Isolated margin, tiered leverage (2x–100x), liquidation by anyone  
**Tier table:**

| Leverage | Init Margin | Maint Margin | Liq Penalty | Funding Mult |
|---|---|---|---|---|
| ≤2x | 50% | 25% | 3% | 1.0x |
| ≤3x | 33% | 17% | 3% | 1.0x |
| ≤5x | 20% | 10% | 5% | 1.5x |
| ≤10x | 10% | 5% | 5% | 2.0x |
| ≤25x | 4% | 2% | 7% | 3.0x |
| ≤50x | 2% | 1% | 10% | 5.0x |
| ≤100x | 1% | 0.5% | 15% | 10.0x |

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Init margin engine |
| **1** | `set_mark_price` | `caller[32]+pair_id[8]+price[8]` | Set oracle mark price (admin/oracle) |
| **2** | `open_position` | `trader[32]+pair_id[8]+side[1]+size[8]+leverage[8]+margin[8]` | Open leveraged position |
| **3** | `close_position` | `caller[32]+pos_id[8]` | Close position (returns margin ± PnL) |
| **4** | `add_margin` | `caller[32]+pos_id[8]+amount[8]` | Top up position margin |
| **5** | `remove_margin` | `caller[32]+pos_id[8]+amount[8]` | Withdraw excess margin (maint check) |
| **6** | `liquidate` | `liquidator[32]+pos_id[8]` | Liquidate underwater position (50% reward) |
| **7** | `set_max_leverage` | `caller[32]+pair_id[8]+max_lev[8]` | Set per-pair max leverage (admin) |
| **8** | `set_maintenance_margin` | `caller[32]+margin_bps[8]` | Override maint margin floor (admin) |
| **9** | `withdraw_insurance` | `caller[32]+amount[8]+recipient[32]` | Withdraw from insurance fund (admin) |
| **10** | `get_position_info` | `pos_id[8]` | Query position details |
| **11** | `get_margin_ratio` | `pos_id[8]` | Query position margin ratio (bps) |
| **12** | `get_tier_info` | `leverage[8]` | Query tier params for leverage level |
| **13** | `emergency_pause` | `caller[32]` | Pause |
| **14** | `emergency_unpause` | `caller[32]` | Unpause |
| **15** | `set_moltcoin_address` | `caller[32]+addr[32]` | Set MOLT token for insurance transfer |

### Trading-Relevant Opcodes
🔴 **Position management:** 2 (open), 3 (close), 4 (add_margin), 5 (remove_margin)  
🔴 **Liquidation:** 6 — anyone can liquidate; 50% penalty → insurance  
🔴 **Price simulation:** 1 (set_mark_price) drives PnL and liquidation  
🔴 **Risk queries:** 11 (margin_ratio), 12 (tier_info)

### E2E Coverage
E2E sends opcodes 0–13. Labels are misaligned from opcode 1:

| E2E Opcode | E2E Label | Actual Function |
|---|---|---|
| 0 | `initialize` | `initialize` ✓ |
| 1 | `set_dex_core_address` | `set_mark_price` ⚠️ |
| 5 | `open_position` | `remove_margin` ⚠️ |
| 8 | `close_position` | `set_maintenance_margin` ⚠️ |

**UNTESTED:** opcodes 14, 15 (`emergency_unpause`, `set_moltcoin_address`)

---

<a id="7-dex_rewards"></a>
## 7. dex_rewards — Trading Incentives, LP Mining & Referrals

**File:** `contracts/dex_rewards/src/lib.rs` (902 lines)  
**Architecture:** Tier-based trading rewards, LP mining, referral program  
**Tiers:** Bronze(1x), Silver(1.5x @ 10k MOLT), Gold(2x @ 100k), Diamond(3x @ 1M)  
**Referral:** Configurable rate (default 10%, max 30% of fees)

### Opcode Dispatch Table

| Op | Function | Signature | Description |
|---|---|---|---|
| **0** | `initialize` | `admin[32]` | Init rewards engine |
| **1** | `record_trade` | `trader[32]+fee_amount[8]+notional[8]` | Record trade for reward calculation |
| **2** | `claim_trading_rewards` | `trader[32]` | Claim pending MOLT rewards |
| **3** | `claim_lp_rewards` | `provider[32]+position_id[8]` | Claim LP mining rewards |
| **4** | `register_referral` | `trader[32]+referrer[32]` | Set referral relationship (once) |
| **5** | `set_reward_rate` | `caller[32]+pair_id[8]+rate[8]` | Set reward rate per pair (admin) |
| **6** | `accrue_lp_rewards` | `position_id[8]+liquidity[8]+pair_id[8]` | Accrue LP rewards for position |
| **7** | `get_pending_rewards` | `addr[32]` | Query pending rewards |
| **8** | `get_trading_tier` | `addr[32]` | Query trader tier (0–3) |
| **9** | `emergency_pause` | `caller[32]` | Pause claims |
| **10** | `emergency_unpause` | `caller[32]` | Unpause |
| **11** | `set_referral_rate` | `caller[32]+rate_bps[8]` | Set referral rate (admin) |
| **12** | `set_moltcoin_address` | `caller[32]+addr[32]` | Set MOLT token for payouts |
| **13** | `set_rewards_pool` | `caller[32]+addr[32]` | Set rewards pool address |
| **14** | `get_referral_rate` | (none) | Query referral rate |
| **15** | `get_total_distributed` | (none) | Query total rewards distributed |

### Trading-Relevant Opcodes
🔴 **Reward accrual:** 1 (record_trade), 6 (accrue_lp_rewards)  
🔴 **Claim paths:** 2 (trading rewards), 3 (LP rewards)  
🔴 **Fee distribution:** Referral bonus computed during record_trade

### E2E Coverage
E2E sends opcodes 0–15. Labels are misaligned from opcode 1:

| E2E Opcode | E2E Label | Actual Function |
|---|---|---|
| 0 | `initialize` | `initialize` ✓ |
| 1 | `set_moltcoin_address` | `record_trade` ⚠️ |
| 5 | `record_trade` | `set_reward_rate` ⚠️ |
| 8 | `claim_trading_rewards` | `get_trading_tier` ⚠️ |

---

<a id="8-moltswap"></a>
## 8. moltswap — Legacy AMM with TWAP Oracle & Flash Loans

**File:** `contracts/moltswap/src/lib.rs` (1308 lines)  
**Architecture:** Constant-product AMM (x·y=k), single pool per instance  
**Special:** No `call()` dispatcher — uses **direct `#[no_mangle]` exports** only  
**v2 features:** TWAP oracle, 5% price impact guard, protocol fee (1/6 of swap fee), flash loans (0.09% fee, 90% max)

### All `#[no_mangle]` Exports

| Function | Line | Description |
|---|---|---|
| `initialize` | 195 | Init pool with token pair addresses |
| `add_liquidity` | 225 | Deposit both tokens, receive LP shares (sqrt-based for first provider) |
| `remove_liquidity` | 263 | Burn LP shares, withdraw proportional tokens |
| `swap_a_for_b` | 306 | Swap token A→B with x·y=k, TWAP update, price impact guard |
| `swap_b_for_a` | 361 | Swap token B→A with x·y=k, TWAP update, price impact guard |
| `swap_a_for_b_with_deadline` | 416 | A→B swap with timestamp deadline |
| `swap_b_for_a_with_deadline` | 426 | B→A swap with timestamp deadline |
| `get_quote` | 436 | Quote swap output without executing |
| `get_reserves` | 448 | Query pool reserves (via output pointers) |
| `get_liquidity_balance` | 462 | Query LP token balance for address |
| `get_total_liquidity` | 474 | Query total LP supply |
| `flash_loan_borrow` | 533 | Borrow up to 90% of reserves; reentrancy guard held until repay |
| `flash_loan_repay` | 586 | Repay loan + fee; fee added to reserves for LPs |
| `flash_loan_abort` | 625 | Abort stale loan after 60s; reserves never modified |
| `get_flash_loan_fee` | 649 | Calculate flash loan fee (0.09%, rounded up) |
| `get_twap_cumulatives` | 660 | Get cumulative TWAP prices + last update timestamp |
| `get_twap_snapshot_count` | 681 | Get TWAP oracle snapshot count |
| `set_protocol_fee` | 693 | Set protocol treasury + fee share (admin) |
| `get_protocol_fees` | 725 | Query accrued protocol fees (token A + B) |
| `set_identity_admin` | 756 | Set identity admin (first-caller-wins) |
| `set_moltyid_address` | 773 | Set MoltyID for reputation lookups |
| `set_reputation_discount` | 797 | Configure reputation-based fee discount |
| `ms_pause` | 870 | Pause all operations (admin) |
| `ms_unpause` | 883 | Unpause (admin) |
| `create_pool` | 900 | **Alias** → `initialize` (test compatibility) |
| `swap` | 906 | **Alias** → dispatches to `swap_a_for_b` or `swap_b_for_a` |
| `get_pool_info` | 916 | Get reserves + total liquidity as return data |
| `get_pool_count` | 934 | Returns 0 or 1 (single-pool AMM) |
| `set_platform_fee` | 940 | **Alias** → `set_protocol_fee` with self as treasury |

### Trading-Relevant Functions
🔴 **Swap execution:** `swap_a_for_b`, `swap_b_for_a`, `swap` (alias)  
🔴 **TWAP oracle:** Updated on every swap; `get_twap_cumulatives`, `get_twap_snapshot_count`  
🔴 **Price queries:** `get_quote`, `get_reserves`  
🔴 **Flash loans:** `flash_loan_borrow`, `flash_loan_repay`, `flash_loan_abort`  
🔴 **Liquidity:** `add_liquidity`, `remove_liquidity`  
🔴 **Fee collection:** `get_protocol_fees`, `set_protocol_fee`

### E2E Coverage (Named Exports)
MoltSwap uses **named function calls** (not opcodes), so the E2E sends function names directly:

| E2E Function | Contract Function | Match? |
|---|---|---|
| `initialize` | `initialize` | ✓ |
| `set_protocol_fee` | `set_protocol_fee` | ✓ |
| `create_pool` | `create_pool` (alias) | ✓ |
| `add_liquidity` | `add_liquidity` | ✓ |
| `remove_liquidity` | `remove_liquidity` | ✓ |
| `swap` | `swap` (alias) | ✓ |
| `get_pool_info` | `get_pool_info` | ✓ |
| `get_pool_count` | `get_pool_count` | ✓ |
| `get_quote` | `get_quote` | ✓ |
| `ms_pause` | `ms_pause` | ✓ |
| `ms_unpause` | `ms_unpause` | ✓ |

✅ **MoltSwap E2E coverage is correct** (named exports match).

**UNTESTED functions:**
- `flash_loan_borrow` / `flash_loan_repay` / `flash_loan_abort` — **flash loans not E2E tested**
- `get_flash_loan_fee`
- `get_twap_cumulatives` / `get_twap_snapshot_count` — **TWAP oracle not E2E tested**
- `swap_a_for_b_with_deadline` / `swap_b_for_a_with_deadline` — deadline swaps not tested
- `get_reserves` / `get_liquidity_balance` / `get_total_liquidity`
- `set_identity_admin` / `set_moltyid_address` / `set_reputation_discount` — MoltyID integration
- `get_protocol_fees`
- `set_platform_fee`

---

<a id="trading-simulation-opcode-map"></a>
## Trading Simulation Opcode Map

Functions directly relevant to trading simulation (orders, swaps, price, OHLCV, TWAP, reserves):

| Category | Contract | Opcode/Fn | Function | Notes |
|---|---|---|---|---|
| **Order Book** | dex_core | 2 | `place_order` | Limit/market/stop-limit/post-only |
| | dex_core | 3 | `cancel_order` | Owner-only cancel |
| | dex_core | 16 | `modify_order` | Atomic cancel+replace |
| | dex_core | 17 | `cancel_all_orders` | Mass cancel on pair |
| **Best Bid/Ask** | dex_core | 10 | `get_best_bid` | Real-time best bid |
| | dex_core | 11 | `get_best_ask` | Real-time best ask |
| | dex_core | 12 | `get_spread` | Bid-ask spread |
| **AMM Swap** | dex_amm | (no opcode) | `swap_exact_in` | Concentrated liquidity swap |
| | dex_amm | (no opcode) | `swap_exact_out` | Exact output swap |
| | dex_amm | (no opcode) | `quote_swap` | Price quote without execution |
| **Legacy Swap** | moltswap | (named) | `swap_a_for_b` | Constant-product AMM swap |
| | moltswap | (named) | `swap_b_for_a` | Constant-product AMM swap |
| | moltswap | (named) | `get_quote` | Swap quote |
| **Routed Swap** | dex_router | 3 | `swap` | Cross-venue routing |
| | dex_router | 9 | `multi_hop_swap` | Multi-hop across pools |
| | dex_router | 5 | `get_best_route` | Route discovery |
| **OHLCV/Candles** | dex_analytics | 1 | `record_trade` | Generates candles for 9 intervals |
| | dex_analytics | 2 | `get_ohlcv` | Query candle data |
| | dex_analytics | 5 | `get_last_price` | Last traded price |
| **TWAP** | moltswap | (named) | `get_twap_cumulatives` | Cumulative oracle prices |
| | moltswap | (named) | `get_twap_snapshot_count` | Snapshot count |
| **Reserves** | moltswap | (named) | `get_reserves` | Pool reserves A/B |
| | dex_amm | (no opcode) | `get_tvl` | AMM pool TVL |
| **Live Price Sim** | dex_analytics | 3 | `get_24h_stats` | 24h high/low/vol/trades |
| | dex_analytics | 4 | `get_trader_stats` | Per-trader PnL |
| | dex_margin | 1 | `set_mark_price` | Oracle mark price |
| **Flash Loans** | moltswap | (named) | `flash_loan_borrow` | 0.09% fee, 90% cap, reentrancy-locked |
| | moltswap | (named) | `flash_loan_repay` | Returns fee to LPs |
| | moltswap | (named) | `flash_loan_abort` | Timeout safety (>60s) |
| **Fee Collection** | dex_core | 15 | `get_fee_treasury` | Protocol fee accumulator |
| | dex_amm | (no opcode) | `collect_fees` | LP fee collection |
| | moltswap | (named) | `get_protocol_fees` | Protocol fee query |
| **Margin Trading** | dex_margin | 2 | `open_position` | 2x–100x leveraged positions |
| | dex_margin | 3 | `close_position` | Returns margin ± PnL |
| | dex_margin | 6 | `liquidate` | Liquidation (50% reward) |
| | dex_margin | 11 | `get_margin_ratio` | Health check |
| **Liquidity Ops** | dex_amm | (no opcode) | `add_liquidity` | Concentrated range |
| | dex_amm | (no opcode) | `remove_liquidity` | Partial/full withdrawal |
| | moltswap | (named) | `add_liquidity` | Proportional deposit |
| | moltswap | (named) | `remove_liquidity` | Proportional withdrawal |

---

<a id="untested-opcodes-summary"></a>
## Untested Opcodes Summary

### Completely Untested (no E2E coverage at all)

| Contract | Opcode/Fn | Function | Risk Level |
|---|---|---|---|
| **dex_core** | 13 | `get_pair_info` | Low |
| **dex_core** | 14 | `get_trade_count` | Low |
| **dex_core** | 15 | `get_fee_treasury` | Medium — fee accounting |
| **dex_core** | 16 | `modify_order` | **HIGH** — cancel+replace atomicity |
| **dex_core** | 17 | `cancel_all_orders` | Medium — mass cancel |
| **dex_core** | 18 | `pause_pair` | Medium — per-pair pause |
| **dex_core** | 19 | `unpause_pair` | Medium — per-pair unpause |
| **dex_core** | 20 | `get_order` | Low |
| **dex_amm** | (none) | `add_liquidity` | **HIGH** — LP deposits |
| **dex_amm** | (none) | `remove_liquidity` | **HIGH** — LP withdrawals |
| **dex_amm** | (none) | `collect_fees` | **HIGH** — fee distribution |
| **dex_amm** | (none) | `swap_exact_in` | **CRITICAL** — swap execution |
| **dex_amm** | (none) | `swap_exact_out` | **CRITICAL** — swap execution |
| **dex_amm** | (none) | `quote_swap` | Medium — price quotes |
| **dex_amm** | (none) | `get_tvl` / `get_pool_info` / etc. | Low |
| **dex_governance** | 14 | `set_moltyid_address` | Low |
| **dex_margin** | 14 | `emergency_unpause` | Low |
| **dex_margin** | 15 | `set_moltcoin_address` | Low |
| **moltswap** | — | `flash_loan_borrow` | **CRITICAL** — flash loan entry |
| **moltswap** | — | `flash_loan_repay` | **CRITICAL** — flash loan exit |
| **moltswap** | — | `flash_loan_abort` | **HIGH** — stale loan cleanup |
| **moltswap** | — | `get_flash_loan_fee` | Low |
| **moltswap** | — | `get_twap_cumulatives` | **HIGH** — TWAP oracle data |
| **moltswap** | — | `get_twap_snapshot_count` | Medium |
| **moltswap** | — | `swap_*_with_deadline` | Medium — deadline swaps |
| **moltswap** | — | `get_reserves` / `get_liquidity_balance` / `get_total_liquidity` | Low |
| **moltswap** | — | `set_identity_admin` / `set_moltyid_address` / `set_reputation_discount` | Low |
| **moltswap** | — | `get_protocol_fees` / `set_platform_fee` | Low |

### Effectively Untested (E2E sends incorrect args due to opcode mismatch)

**ALL opcode-dispatched contracts** (dex_core, dex_amm, dex_router, dex_analytics, dex_governance, dex_margin, dex_rewards) have their opcodes 1+ exercised with **wrong argument payloads**, causing silent failures in arg-length guards.

The only contract with **reliable E2E coverage** is **moltswap** (uses named exports, not opcodes).

---

## Recommendations

1. **Fix dex_amm dispatcher:** Add opcodes 2–14 to the `call()` function to expose `add_liquidity`, `remove_liquidity`, `swap_exact_in`, `swap_exact_out`, `collect_fees`, `emergency_pause/unpause`, and all query functions.

2. **Realign E2E opcode map:** Update `build_opcode_scenarios()` in `comprehensive-e2e-parallel.py` to use the correct opcode numbers matching each contract's actual `call()` dispatch table.

3. **Add flash loan E2E tests:** `flash_loan_borrow` → `flash_loan_repay` cycle and `flash_loan_abort` timeout are untested end-to-end.

4. **Add TWAP E2E tests:** `get_twap_cumulatives` after multiple swaps to verify oracle accumulation.

5. **Add OHLCV E2E tests:** `record_trade` with multiple prices → `get_ohlcv` to verify candle generation across intervals.

6. **Add modify_order E2E test:** Opcode 16 (cancel+replace) has atomicity guarantees that need E2E verification.

7. **Verify margin liquidation E2E:** `set_mark_price` → `open_position` → price move → `liquidate` path needs end-to-end coverage with the actual opcode numbers.
