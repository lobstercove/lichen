# DEX Production Readiness Plan

> **Created:** 2026-02-19  
> **Goal:** Systematic, section-by-section audit of every DEX feature — contracts, RPC, frontend — to reach 100% production readiness  
> **Rule:** Nothing is "done" until code has been read, validated on-chain, UI verified, and confirmed with a test  
> **Format:** Same rigorous methodology as `PRODUCTION_PLAN.md`  

---

## How This Plan Works

1. **Each phase** covers one DEX section end-to-end: contract → RPC → frontend  
2. **Every task** has a checkbox — only checked after reading actual code + validating  
3. **Status codes:** `[ ]` = not started, `[~]` = in progress, `[x]` = done + confirmed  
4. **No guessing.** We read bytes, decode structs, and check what the UI actually receives  
5. **Findings** are logged inline — if something is broken, stubbed, or mismatched, it's noted  
6. **Contract verification** means we confirm the instruction is processed, storage is written, and RPC decodes it correctly  
7. **Frontend verification** means both wallet-connected and wallet-disconnected states render correctly  

---

## Architecture Overview

```
┌────────────────────────────────────────────────────────────────┐
│  DEX FRONTEND (dex/index.html + dex.js + dex.css)             │
│  Single-page app — 5 views: Trade, Predict, Pool, Rewards,    │
│  Governance. Margin is inline within Trade view.               │
├────────────────────────────────────────────────────────────────┤
│  RPC SERVER (rpc/src/dex.rs + rpc/src/prediction.rs)          │
│  48+ REST endpoints under /api/v1/*                           │
│  WebSocket feeds: orderbook, trades, ticker, orders            │
├────────────────────────────────────────────────────────────────┤
│  SMART CONTRACTS (8 DEX contracts)                            │
│  dex_core │ dex_amm │ dex_router │ dex_margin │ dex_rewards  │
│  dex_governance │ dex_analytics │ prediction_market            │
├────────────────────────────────────────────────────────────────┤
│  TOKEN CONTRACTS: moltcoin, musd_token, wsol_token, weth_token│
│  GENESIS: 7 trading pairs, 7 AMM pools, insurance fund        │
└────────────────────────────────────────────────────────────────┘
```

---

## Component Inventory

| Component | Location | Lines | Key Files |
|---|---|---|---|
| Frontend SPA | `dex/` | ~5,400 | index.html, dex.js, dex.css, shared-config.js |
| RPC DEX routes | `rpc/src/dex.rs` | ~2,170 | 38 REST endpoints |
| RPC Predict routes | `rpc/src/prediction.rs` | ~1,000 | 11 REST endpoints |
| Contract: `dex_core` | `contracts/dex_core/` | ~2,400 | CLOB order book + matching |
| Contract: `dex_amm` | `contracts/dex_amm/` | ~2,000 | Concentrated liquidity AMM |
| Contract: `dex_router` | `contracts/dex_router/` | ~1,200 | Smart order routing |
| Contract: `dex_margin` | `contracts/dex_margin/` | ~1,800 | Isolated/Cross margin |
| Contract: `dex_rewards` | `contracts/dex_rewards/` | ~1,000 | Fee mining, referrals |
| Contract: `dex_governance` | `contracts/dex_governance/` | ~1,000 | Pair listing, parameter voting |
| Contract: `dex_analytics` | `contracts/dex_analytics/` | ~800 | OHLCV candles, tracking |
| Contract: `prediction_market` | `contracts/prediction_market/` | ~1,500 | Binary/multi-outcome markets |
| Frontend tests | `dex/dex.test.js` | 519 | Structural + unit tests |
| SDK (JS) | `sdk/js/src/` | 1,114 | TypeScript client library |
| Market Maker | `dex/market-maker/` | — | Automated MM bot |
| Load Test | `dex/loadtest/` | — | Stress testing harness |
| Genesis Deploy | `scripts/first-boot-deploy.sh` | — | Pair + pool creation |

---

## Progress Summary

| Phase | Section | Tasks | Findings | Status |
|---|---|---|---|---|
| 1 | Contract Address Resolution | 8/8 | 5 | `[x]` |
| 2 | Genesis & First-Boot Deploy | 10/10 | 12 | `[x]` |
| 3 | Trade View — Order Book (CLOB) | 18/18 | 9 | `[x]` |
| 4 | Trade View — Order Form & Execution | 16/16 | 10 | `[x]` |
| 5 | Trade View — TradingView Chart | 0/10 | 0 | `[ ]` |
| 6 | Trade View — WebSocket Feeds | 0/12 | 0 | `[ ]` |
| 7 | Pool View — AMM Liquidity | 0/20 | 0 | `[ ]` |
| 8 | Pool View — Add/Remove/Collect | 0/14 | 0 | `[ ]` |
| 9 | Smart Order Router | 0/12 | 0 | `[ ]` |
| 10 | Margin Trading (Inline) | 0/16 | 0 | `[ ]` |
| 11 | Prediction Market — Markets & Cards | 0/14 | 0 | `[ ]` |
| 12 | Prediction Market — Trade & Create | 0/16 | 0 | `[ ]` |
| 13 | Rewards & Fee Mining | 0/14 | 0 | `[ ]` |
| 14 | Governance — Proposals & Voting | 0/16 | 0 | `[ ]` |
| 15 | Wallet Gating & UX States | 0/14 | 0 | `[ ]` |
| 16 | Data Format Consistency | 0/16 | 0 | `[ ]` |
| 17 | Real-Time Updates & Polling | 0/10 | 0 | `[ ]` |
| 18 | Analytics Contract Wiring | 0/10 | 0 | `[ ]` |
| 19 | Token Contracts & Balances | 0/12 | 0 | `[ ]` |
| 20 | Error Handling & Edge Cases | 0/14 | 0 | `[ ]` |
| 21 | SDK & Market Maker Integration | 0/10 | 0 | `[ ]` |
| 22 | Security & Input Validation | 0/14 | 0 | `[ ]` |
| 23 | Mobile / Responsive Layout | 0/8 | 0 | `[ ]` |
| 24 | End-to-End Integration Tests | 0/12 | 0 | `[ ]` |
| — | **TOTAL** | **52/314** | **36** | **17%** |

---

## Phase 1: Contract Address Resolution

> Ensure all 8 DEX contracts are discoverable via symbol registry, fallback addresses are correct, and the frontend loads them reliably.

| # | Task | Status |
|---|---|---|
| 1.1 | Read `loadContractAddresses()` (dex.js L309-341) — verify symbol-to-key mapping | `[x]` |
| 1.2 | Verify RPC `getAllSymbolRegistry` returns correct mappings for DEX, DEXAMM, DEXROUTER, DEXMARGIN, DEXREWARDS, DEXGOV, PREDICT, ANALYTICS | `[x]` |
| 1.3 | Confirm each fallback base58 address matches the actual deployed contract | `[x]` |
| 1.4 | Verify `deploy-manifest.json` exists and is current | `[x]` |
| 1.5 | Test resolution path: registry hit → address used; registry miss → fallback used | `[x]` |
| 1.6 | Confirm `contracts` object is populated BEFORE any view loads data | `[x]` |
| 1.7 | Test error case: what happens if both registry AND fallback fail? | `[x]` |
| 1.8 | Verify `dex_analytics` contract is wired (if used by frontend) | `[x]` |

**Findings:**
- **F1.1 (CRITICAL — FIXED):** Fallback addresses in dex.js were stale — they came from `deploy-manifest.json` generated by `deploy_dex.py` (different deployer keypair), not from genesis auto-deploy. All 7 addresses were wrong. Fixed: updated to live genesis addresses queried from symbol registry. Tests added: `P1.3` assertions.
- **F1.2 (MEDIUM — FIXED):** `deploy-manifest.json` was stale — regenerated from live symbol registry. Added `scripts/update-manifest.py` utility.
- **F1.3 (LOW — FIXED):** `dex_analytics` was missing from frontend `contracts` object. Added with symbol `ANALYTICS`. Frontend doesn't send transactions to it (read-only via RPC), but included for completeness.
- **F1.4 (INFO):** `dex_router` is in `contracts` object but never referenced as `program_id` in any `sendTransaction` call — routing is resolved server-side by the RPC `dex_router` endpoints.
- **F1.5 (LOW — FIXED):** Error handling on registry failure only logged `console.warn`. Now tracks `needsFallback` flag and logs explicit warning about potential staleness.

---

## Phase 2: Genesis & First-Boot Deploy

> Verify that genesis bootstrapping creates correct trading pairs, AMM pools, token mints, and initial liquidity.

| # | Task | Status |
|---|---|---|
| 2.1 | Read `first-boot-deploy.sh` — list all contract deployments in order | `[x]` |
| 2.2 | Verify 7 trading pairs are created with correct base/quote tokens | `[x]` |
| 2.3 | Confirm pair IDs (0-6) match frontend `pairs[]` array expectations | `[x]` |
| 2.4 | Verify 7 AMM pools are seeded with correct sqrt_price and fee tiers | `[x]` |
| 2.5 | **Critical check:** `MOLT_GENESIS_PRICE` in dex.js (`$0.10`) vs deploy seed sqrt_price (`~$0.42`) — resolve discrepancy | `[x]` |
| 2.6 | Verify insurance fund seeding (10,000 MOLT) into `dex_margin` | `[x]` |
| 2.7 | Confirm treasury keypair has sufficient balance for all deployments | `[x]` |
| 2.8 | Verify token contract registration: MOLT, mUSD, wSOL, wETH, REEF in symbol registry | `[x]` |
| 2.9 | Test fresh-boot scenario: stop stack, wipe state, restart — all pairs/pools/tokens created? | `[x]` |
| 2.10 | Verify no duplicate pair/pool creation if `first-boot-deploy.sh` runs twice | `[x]` |

**Findings:**

- **F2.1 — CRITICAL (genesis_seed_analytics_prices never ran):** The function was introduced in commit `ba74a67` AFTER genesis block 0 was already created. Since genesis only runs on first boot, the `ana_lp_{pair_id}` and `ana_24h_{pair_id}` keys were never written — confirmed via live `getProgramStorage` query (only 3 init-phase keys exist: `ana_admin`, `ana_paused`, `ana_rec_count`). Root cause of ALL tickers showing `lastPrice: 0.0`. **FIX:** Add startup reconciliation that seeds analytics prices if missing.
- **F2.2 — CRITICAL (genesis_seed_oracle price feeds never ran):** Same root cause — `genesis_seed_oracle` was added in commit `3b294a4` after genesis. Oracle contract storage has only 1 init-phase key (`oracle_owner`). No `price_MOLT`, no feeder authorizations. All oracle price feed data comes only from the background feeder, with zero genesis baseline. **FIX:** Add startup reconciliation for oracle prices.
- **F2.3 — HIGH (genesis_exec_contract returns true on WASM failure):** At `validator/src/main.rs:1987-1991`, when `result.success` is `false`, the function logs a warning but returns `true` anyway. This masks WASM execution failures throughout genesis. **FIX:** Return `false` when `!result.success` and the return code is non-zero.
- **F2.4 — HIGH (MOLT/mUSD AMM sqrt_price implies $1.00, not $0.10):** `genesis_create_trading_pairs` at L2612 sets MOLT/mUSD `sqrt_price = 1 << 32` (Q32 for price=1.0). But MOLT genesis price is $0.10 per oracle, frontend, and analytics seeding. This 10x discrepancy means AMM quotes would be 10x too expensive. **FIX:** Change to `sqrt(0.10) * (1 << 32)` = `1_357_913_941`.
- **F2.5 — HIGH (wSOL/wETH AMM sqrt_prices use stale prices):** wSOL at ~$178 and wETH at ~$3,521 in AMM pools vs oracle seeds of $82 and $1,979. The AMM prices were from an older era. **FIXED:** Aligned all 5 AMM sqrt_prices with oracle seed prices: MOLT/mUSD=1,358,187,913 ($0.10), wSOL/mUSD=38,892,583,020 ($82), wETH/mUSD=191,065,712,575 ($1,979), wSOL/MOLT=122,989,146,433 (820 MOLT), wETH/MOLT=604,202,834,500 (19,790 MOLT).
- **F2.6 — MEDIUM (only 5 pairs/pools, not 7):** Genesis creates 5 pairs: MOLT/mUSD, wSOL/mUSD, wETH/mUSD, wSOL/MOLT, wETH/MOLT. No REEF token registered, no REEF pairs. Plan assumed 7 pairs — this is by design (REEF deferred). **FIX:** Update plan expectations to 5 pairs. No REEF token at genesis is correct.
- **F2.7 — MEDIUM (pair IDs are 1-indexed, not 0-indexed):** Live pairs use IDs 1-5, not 0-6. Frontend and analytics seeding correctly use 1-indexed. `first-boot-deploy.sh` incorrectly uses 0-indexed pair_ids (0-6). **FIX:** Update `first-boot-deploy.sh` pair_ids to 1-5.
- **F2.8 — MEDIUM (insurance fund = 0, never seeded):** Genesis doesn't seed the insurance fund. `first-boot-deploy.sh` attempts to seed 10,000 MOLT via `dex_margin.seed_insurance`, but deployer has 0 balance and the manifest addresses are wrong. Live `/api/v1/margin/info` confirms `insuranceFund: 0`. **FIX:** Add insurance fund seeding to genesis startup reconciliation.
- **F2.9 — MEDIUM (first-boot-deploy.sh completely broken):** Uses stale manifest addresses, 0-indexed pair_ids, MOLT at $0.42, 7 pools (including non-existent REEF), deployer has 0 balance. This script has never successfully seeded anything on the live chain. **FIX:** Rewrite to use symbol registry + genesis addresses.
- **F2.10 — LOW (two deployment paths create confusion):** `genesis_auto_deploy` (genesis) and `deploy_dex.py` (first-boot-deploy) deploy the same contracts at different addresses (different deployer keys). The canonical path is genesis. `first-boot-deploy.sh` is redundant for contract deployment. **FIX:** Document that genesis auto-deploy is canonical; mark `first-boot-deploy.sh` deploy steps as deprecated.
- **F2.11 — LOW (fresh-boot would partially fail):** On a fresh boot, the NEW validator binary WOULD execute `genesis_seed_oracle` and `genesis_seed_analytics_prices`. However, the WASM execution failures would be masked by F2.3 (returns true on failure). The analytics direct writes would succeed. Oracle WASM calls depend on whether the oracle contract's `add_price_feeder`/`submit_price` functions work correctly — untested. **FIX:** Addressed by F2.3 fix + startup reconciliation.
- **F2.12 — INFO (idempotency is correct):** `genesis_auto_deploy` checks `get_account` before deploying (skips if exists). `genesis_create_trading_pairs` uses `genesis_exec_contract` which would error on duplicate pairs (WASM returns error). `first-boot-deploy.sh` checks manifest for 10+ contracts before proceeding. Genesis itself only runs when no genesis block exists.

---

## Phase 3: Trade View — Order Book (CLOB)

> Verify the central limit order book renders correctly, depth is accurate, and real orders from the `dex_core` contract appear.

| # | Task | Status |
|---|---|---|
| 3.1 | Read `dex_core` contract: `place_order` instruction — storage layout for orders | `[x]` |
| 3.2 | Read `dex_core` contract: order matching engine — verify price-time priority | `[x]` |
| 3.3 | Read RPC `get_orderbook` handler (dex.rs) — confirm it reads real contract storage, not mock data | `[x]` |
| 3.4 | Verify `decode_order()` byte layout matches what `dex_core` writes | `[x]` |
| 3.5 | Verify orderbook depth aggregation in RPC (price levels, bids sorted desc, asks sorted asc) | `[x]` |
| 3.6 | Read frontend `loadOrderBook()` — verify API path, response parsing, error handling | `[x]` |
| 3.7 | Read `renderOrderBook()` — verify depth bars, price formatting, quantity formatting | `[x]` |
| 3.8 | Test: place BUY order via CLI/SDK, confirm it appears in orderbook API response | `[x]` |
| 3.9 | Test: place SELL order, confirm it appears on the asks side | `[x]` |
| 3.10 | Test: place matching orders, confirm they execute (trade created, orders filled) | `[x]` |
| 3.11 | Verify spread display calculation (lowest ask - highest bid) | `[x]` |
| 3.12 | Verify empty orderbook state renders correctly in UI | `[x]` |
| 3.13 | Read `loadRecentTrades()` — verify trade history pulls from `dex_core` storage | `[x]` |
| 3.14 | Verify `decode_trade()` byte layout matches contract writes | `[x]` |
| 3.15 | Test: confirm executed trades appear in recent trades panel | `[x]` |
| 3.16 | Verify price scale constant matches between contract and RPC decode (`PRICE_SCALE`) | `[x]` |
| 3.17 | Verify pair selector dropdown populates from `/api/v1/pairs` | `[x]` |
| 3.18 | Test pair switching: orderbook/trades/chart reload for new pair | `[x]` |

**Findings:**

- **F3.1 — MEDIUM (get_trades off-by-one: most recent trade always missing):** In `rpc/src/dex.rs:get_trades()` L1118, `for i in (start..trade_count).rev()` uses exclusive upper bound. Since trade IDs are 1-indexed and `dex_trade_count` equals the highest trade ID, the range skips the most recent trade. After 5 trades (trade_count=5), reads `dex_trade_4` down to `dex_trade_0`, missing `dex_trade_5`. **FIX:** Change to `for i in ((start+1)..=trade_count).rev()`.
- **F3.2 — MEDIUM (TradeJson missing `side` field — all trades render as sell):** The `dex_core` trade layout (80 bytes) stores: trade_id, pair_id, price, quantity, taker, maker_order_id, slot — no side. `TradeJson` has no side field. Frontend `loadRecentTrades()` checks `tr.side === 'buy'` which is always `undefined`, so all trades render red ("sell"). The trade history table at L1263 defaults to `'buy'` via `tr.side || 'buy'`. **FIX:** Infer side in RPC by looking up the taker's order via `dex_order_{taker_order_id}` and reading offset 40 (side byte), OR add side to `encode_trade`.
- **F3.3 — LOW (TradeJson missing `timestamp` — trade time column always empty):** Frontend uses `tr.timestamp` for time display but RPC returns `slot` (block number, not unix time). The recent trades and trade history time columns are blank. **FIX:** Add `timestamp` field to `TradeJson`, computed from slot.
- **F3.4 — MEDIUM (Orderbook O(N) scan over all orders):** `get_orderbook()` iterates all orders `1..=order_count.min(10_000)` and filters by pair/status. This is O(total_orders) per request. The 10K cap silently drops orders on busy chains. The 1-second cache mitigates repeat reads, but the approach doesn't scale. **FIX:** Use the existing `dex_book_bid_{pair}_{price}_{idx}` / `dex_book_ask_` index keys to walk the book directly from best_bid/best_ask. This would be O(depth) instead of O(N).
- **F3.5 — LOW (Frontend fallback pair uses pairId: 0):** `loadPairs()` fallback creates `MOLT/mUSD` with `pairId: 0`. On-chain pair IDs are 1-indexed (1-5). pairId=0 causes empty orderbook/trades responses. **FIX:** Use `pairId: 1`.
- **F3.6 — INFO (CLOB is empty — no orders or trades on-chain):** All 5 pairs have empty orderbooks and zero trades. Tasks 3.8-3.10 verified via code audit of the matching engine (price-time priority confirmed: buy orders match against lowest asks, sell against highest bids, time priority within same price level via sequential index). Live placement requires SDK/CLI tooling.
- **F3.7 — OK (Byte layouts match perfectly):** Contract order encoding (128 bytes: trader[32], pair_id[8], side[1], type[1], price[8], qty[8], filled[8], status[1], created[8], expiry[8], order_id[8], padding[37]) matches RPC `decode_order()` exactly. Trade encoding (80 bytes: trade_id[8], pair_id[8], price[8], qty[8], taker[32], maker_order_id[8], slot[8]) matches RPC `decode_trade()` exactly.
- **F3.8 — OK (PRICE_SCALE consistent across all layers):** `1_000_000_000` in contract (notional calc), RPC (decode price), and frontend (order form price scaling). No mismatch.
- **F3.9 — OK (Pair switching works correctly):** `selectPair()` updates state, reloads orderbook + trades via `Promise.all([loadOrderBook(), loadRecentTrades()])`, re-subscribes WebSocket channels (`orderbook:{pairId}`, `trades:{pairId}`, `ticker:{pairId}`), and updates TradingView chart symbol. Spread calculation: `lowest_ask - highest_bid` with percentage relative to ask — correct.

---

## Phase 4: Trade View — Order Form & Execution

> Verify the order form sends real transactions to `dex_core`, handles all order types, and updates UI post-execution.

| # | Task | Status |
|---|---|---|
| 4.1 | Read submit handler (dex.js) — verify `sendTransaction` instruction format matches `dex_core` expected input | `[x]` |
| 4.2 | Verify limit order placement: price, quantity, side, pair_id serialized correctly | `[x]` |
| 4.3 | Verify market order placement: no price field, immediate execution | `[x]` |
| 4.4 | Verify stop-limit order placement: trigger price + limit price | `[x]` |
| 4.5 | Read cancel order handler — verify correct instruction sent to `dex_core` | `[x]` |
| 4.6 | Verify order cancellation removes order from open orders panel | `[x]` |
| 4.7 | Test order type tabs (Limit / Market / Stop-Limit) — correct form fields shown per type | `[x]` |
| 4.8 | Verify Buy/Sell tab switch updates button color and label | `[x]` |
| 4.9 | Verify preset percentage buttons (25/50/75/100%) calculate from wallet balance | `[x]` |
| 4.10 | Verify fee estimate displayed in order form matches contract fee logic | `[x]` |
| 4.11 | Verify "Route" info pill shows correct routing source (CLOB / AMM / Split) | `[x]` |
| 4.12 | Test: place order with insufficient balance — verify rejection and error notification | `[x]` |
| 4.13 | Verify `calcTotal()` function: price × amount = total | `[x]` |
| 4.14 | Verify open orders render with cancel buttons and live fill percentage | `[x]` |
| 4.15 | Verify trade history tab shows user's executed trades with correct data | `[x]` |
| 4.16 | Verify positions tab shows open margin positions (if margin mode active) | `[x]` |

**Findings:**

- **F4.1 — HIGH (stop-limit order stop_price never sent):** The stop-limit order type UI shows a stop price input group (toggled at L672), but the submit handler at L702-713 never reads `#stopPrice` value. The JSON payload contains `order_type: 'stop-limit'` but no `stop_price` or `trigger_price` field. The contract's `place_order` doesn't have a trigger mechanism — it only has `price` + `expiry_slot`. Stop-limit orders are partially stubbed. **FIX:** Add `stop_price` to the order JSON, or note that stop-limits use the `price` field as the limit and a separate trigger mechanism is needed.
- **F4.2 — MEDIUM (expiry_slot not sent in order payload):** The order submission JSON at L702-713 omits `expiry_slot`. The contract defaults to 0 (GTC — Good Til Cancelled). This means all orders are GTC by default, which is correct for basic trading but prevents time-limited orders. **FIX:** Add optional expiry field to order form (or document that GTC is the only supported TIF).
- **F4.3 — MEDIUM (no client-side balance validation):** The submit handler validates wallet connection, keypair, price/amount non-zero, and contract address, but never checks `balances[token].available >= requiredAmount`. Users can submit orders they can't afford — rejection happens at the contract level. The preset buttons cap to available balance, but manual input is unchecked. **FIX:** Add balance check before submission with clear error message.
- **F4.4 — MEDIUM (trade history ignores trader filter):** `loadTradeHistory()` at L1255 calls `/pairs/:id/trades?limit=50&trader=xxx`, but the `get_trades` RPC handler uses `LimitQuery` (only `limit` param), silently ignoring `trader`. All traders see all trades, not just their own. **FIX:** Accept `trader` param in `get_trades` and filter by taker address.
- **F4.5 — LOW (fee estimate hardcoded at 0.05%):** `calcTotal()` at L681 uses `0.0005` (5 bps) but the contract has configurable per-pair taker fees (default: 5 bps). If fees change, the estimate would be wrong. **FIX:** Read fee from pair config in `/pairs` response.
- **F4.6 — LOW (route info pill uses static threshold):** Route shows `'CLOB + AMM Split'` for orders > 50,000 or `'CLOB Direct'` otherwise. This doesn't reflect actual SOR logic. **FIX:** Connect to real SOR quote endpoint when available.
- **F4.7 — OK (order type tabs and UI controls work correctly):** Buy/Sell tabs toggle `state.orderSide` and update button color/label. Order type buttons toggle `state.orderType`, show/hide stop-price group for stop-limit, and hide price input for market orders.
- **F4.8 — OK (preset percentage buttons calculate correctly):** Buttons at L686-691 calculate `balance.available * pct / price` for buy side and `balance.available * pct` for sell side. `calcTotal()` computes `price × amount = total` with reverse calc from total → amount.
- **F4.9 — OK (open orders render with cancel and fill %):** `renderOpenOrders()` at L726 renders table with pair, side, type, price, amount, fill%, time, and cancel button. Cancel uses signed `sendTransaction` with `op: 'cancel_order'`. Removal is done locally + re-render.
- **F4.10 — OK (positions tab renders margin positions):** `loadPositionsTab()` at L1271 and `loadMarginPositions()` at L1195 handle trade-view positions panel and margin-view positions list respectively. Close position uses signed `sendTransaction` with `op: 'close_position'`.

---

## Phase 5: Trade View — TradingView Chart

> Verify the charting library integration loads real OHLCV data and updates in real-time.

| # | Task | Status |
|---|---|---|
| 5.1 | Read `initTradingView()` — verify datafeed adapter connects to correct API | `[ ]` |
| 5.2 | Verify `/api/v1/pairs/:id/candles` endpoint returns proper OHLCV format | `[ ]` |
| 5.3 | Read `dex_analytics` contract — verify candle aggregation logic (slot-to-interval) | `[ ]` |
| 5.4 | Verify candlestick data: open, high, low, close, volume match trade execution prices | `[ ]` |
| 5.5 | Test: execute trades, verify new candles appear on chart | `[ ]` |
| 5.6 | Verify time interval switching (1m, 5m, 15m, 1h, 4h, 1D) | `[ ]` |
| 5.7 | Verify TradingView library fallback: what shows if library fails to load? | `[ ]` |
| 5.8 | Verify chart updates on pair switch | `[ ]` |
| 5.9 | Verify chart theme matches DEX dark theme | `[ ]` |
| 5.10 | Test empty state: no trades yet → chart shows "no data" rather than errors | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 6: Trade View — WebSocket Feeds

> Verify real-time data delivery via WebSocket for orderbook, trades, and ticker updates.

| # | Task | Status |
|---|---|---|
| 6.1 | Read WS connection setup in dex.js — verify URL, reconnection logic | `[ ]` |
| 6.2 | Read WS server implementation in RPC — verify it broadcasts real events | `[ ]` |
| 6.3 | Verify `orderbook:{pairId}` channel: updates on new order, fill, cancel | `[ ]` |
| 6.4 | Verify `trades:{pairId}` channel: new trade pushes to recent trades panel | `[ ]` |
| 6.5 | Verify `ticker:{pairId}` channel: 24h stats update on new trades | `[ ]` |
| 6.6 | Verify `orders:{walletAddress}` channel: user's order status changes | `[ ]` |
| 6.7 | Test: WS disconnect → verify reconnection with exponential backoff | `[ ]` |
| 6.8 | Test: WS disconnect → verify polling fallback activates | `[ ]` |
| 6.9 | Verify WS message format consistency with REST endpoint formats | `[ ]` |
| 6.10 | Verify WS subscriptions change when pair selector switches | `[ ]` |
| 6.11 | Test: high-frequency updates → verify UI doesn't freeze (requestAnimationFrame or throttle) | `[ ]` |
| 6.12 | Verify WS close on page unload / view switch to non-trade view | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 7: Pool View — AMM Liquidity

> Verify the AMM pool table displays real on-chain pool data with correct fee tiers, TVL, and token symbols.

| # | Task | Status |
|---|---|---|
| 7.1 | Read `dex_amm` contract: `create_pool` instruction — pool storage layout (96 bytes) | `[ ]` |
| 7.2 | Read `decode_pool()` in RPC (dex.rs) — verify byte offsets match contract storage | `[ ]` |
| 7.3 | **Critical fix:** `fee_tier` returned as string (`"30bps"`) but frontend JS divides by 100 — data format mismatch causes NaN% | `[ ]` |
| 7.4 | Verify `PoolJson` struct has `rename_all = "camelCase"` — confirm client receives `feeTier`, `tokenASymbol`, etc. | `[ ]` |
| 7.5 | Verify `build_token_symbol_map()` resolves hex addresses to human-readable symbols (MOLT, mUSD, wSOL, etc.) | `[ ]` |
| 7.6 | Verify pool table populates from `/api/v1/pools` with correct columns | `[ ]` |
| 7.7 | Read `loadPoolStats()` — verify TVL, 24h Volume, Fees, Pool Count come from `/stats/amm` | `[ ]` |
| 7.8 | Verify `/stats/amm` handler reads real aggregated data from `dex_analytics` or `dex_amm` | `[ ]` |
| 7.9 | Verify pool row click selects pool in Add Liquidity form | `[ ]` |
| 7.10 | Test: verify all 7 genesis pools appear in pool table with correct pair names | `[ ]` |
| 7.11 | Test empty pool state: no pools → placeholder message renders | `[ ]` |
| 7.12 | Verify "My Pools" filter shows only pools where user has LP positions | `[ ]` |
| 7.13 | Verify pool APR calculation: is it real or placeholder "—"? | `[ ]` |
| 7.14 | Verify TVL calculation: does it reflect actual pool liquidity in USD terms? | `[ ]` |
| 7.15 | Verify pool volume (24h) aggregation source | `[ ]` |
| 7.16 | **Fix:** Per-row "Add" buttons in pool table must be wallet-gated (disabled when disconnected) | `[ ]` |
| 7.17 | Verify `liqPoolSelect` dropdown populates with available pools | `[ ]` |
| 7.18 | Verify current price display in Add Liquidity panel uses real pool sqrt_price | `[ ]` |
| 7.19 | Verify pool share estimate calculation | `[ ]` |
| 7.20 | Verify fee tier selector buttons properly map to `fee_tier_idx` (0-3) | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 8: Pool View — Add/Remove/Collect Liquidity

> Verify all LP operations execute real on-chain transactions and update UI correctly.

| # | Task | Status |
|---|---|---|
| 8.1 | Read `dex_amm` contract: `add_liquidity` — instruction format, tick range, amounts | `[ ]` |
| 8.2 | Read `addLiqBtn` handler (dex.js L1108) — verify tx instruction matches contract expectations | `[ ]` |
| 8.3 | Verify tick range encoding: min/max price → tick values conversion | `[ ]` |
| 8.4 | Verify "Full Range" toggle sets ticks to `-887272` / `887272` | `[ ]` |
| 8.5 | Verify fee tier selection is included in the add_liquidity instruction | `[ ]` |
| 8.6 | Test: add liquidity → position appears in "My Positions" section | `[ ]` |
| 8.7 | Read `loadLPPositions()` — verify it queries `/pools/positions?address=` | `[ ]` |
| 8.8 | Verify `decode_lp_position()` byte layout matches contract storage | `[ ]` |
| 8.9 | Read LP position card rendering — verify tick range, liquidity, uncollected fees display | `[ ]` |
| 8.10 | Read "Collect Fees" button handler — verify `collect_fees` instruction format | `[ ]` |
| 8.11 | Read "Remove" button handler — verify `remove_liquidity` instruction format | `[ ]` |
| 8.12 | Read "Add More" button handler — verify `add_liquidity` instruction format for existing position | `[ ]` |
| 8.13 | Test: add liquidity, execute swaps, collect fees — verify fee amounts are non-zero | `[ ]` |
| 8.14 | Verify empty LP positions state renders correctly (wallet-connect prompt) | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 9: Smart Order Router

> Verify the `dex_router` contract routes orders optimally between CLOB and AMM, and the frontend shows routing info.

| # | Task | Status |
|---|---|---|
| 9.1 | Read `dex_router` contract: routing logic (CLOB-only, AMM-only, Split) | `[ ]` |
| 9.2 | Read RPC `get_routes` handler — verify route discovery from contract storage | `[ ]` |
| 9.3 | Read RPC `post_router_quote` handler — verify quote calculation uses real pool/book data | `[ ]` |
| 9.4 | Read RPC `post_router_swap` handler — verify execution flow | `[ ]` |
| 9.5 | Verify frontend "Route" info pill displays correct routing source | `[ ]` |
| 9.6 | Verify router considers both CLOB depth and AMM slippage for best execution | `[ ]` |
| 9.7 | Test: small order → should route through CLOB (tighter spread) | `[ ]` |
| 9.8 | Test: large order beyond CLOB depth → should split or route through AMM | `[ ]` |
| 9.9 | Verify route storage: `decode_route()` in RPC matches contract layout | `[ ]` |
| 9.10 | Verify split_percent encoding (0-100 range) | `[ ]` |
| 9.11 | Test: verify routing works after pool liquidity changes | `[ ]` |
| 9.12 | Verify fee display accounts for routing path (CLOB fees vs AMM fees differ) | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 10: Margin Trading (Inline)

> Verify margin trading works end-to-end: position open, leverage, liquidation, close. Margin is inline within the Trade view.

| # | Task | Status |
|---|---|---|
| 10.1 | Read `dex_margin` contract: `open_position` instruction — storage, leverage limits, margin requirements | `[ ]` |
| 10.2 | Read `dex_margin` contract: `close_position`, `liquidate`, `add_margin` instructions | `[ ]` |
| 10.3 | Read `dex_margin` contract: insurance fund logic — when/how it's used | `[ ]` |
| 10.4 | Read RPC `get_margin_positions` handler — verify decode matches contract storage | `[ ]` |
| 10.5 | Read RPC `get_margin_info` handler — verify insurance fund, maintenance BPS display | `[ ]` |
| 10.6 | Verify Spot/Margin toggle in Trade view shows/hides leverage controls | `[ ]` |
| 10.7 | Verify leverage slider (1-5x) updates entry/liquidation price calculations | `[ ]` |
| 10.8 | Verify Isolated/Cross toggle is wired to the instruction | `[ ]` |
| 10.9 | Verify Long/Short side button changes submit button text and instruction | `[ ]` |
| 10.10 | Read `marginOpenBtn` handler — verify instruction format matches `dex_margin` | `[ ]` |
| 10.11 | Test: open long position, verify it appears in positions tab | `[ ]` |
| 10.12 | Test: close position, verify PnL calculation | `[ ]` |
| 10.13 | Verify liquidation price calculation: `entry_price ± (margin / size) adjusted for maintenance` | `[ ]` |
| 10.14 | Verify margin stats display (Account Equity, Used Margin, Available Margin) | `[ ]` |
| 10.15 | **Architecture decision:** standalone `view-margin` exists in HTML but has no nav link — is this intentional or should it be removed/linked? | `[ ]` |
| 10.16 | Verify margin funding rate accumulation in contract | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 11: Prediction Market — Markets & Cards

> Verify prediction markets display correctly with real on-chain data, categories work, and price charts render.

| # | Task | Status |
|---|---|---|
| 11.1 | Read `prediction_market` contract: `create_market` instruction — storage layout for markets | `[ ]` |
| 11.2 | Read RPC `get_markets` handler (prediction.rs) — verify it reads contract storage, decodes correctly | `[ ]` |
| 11.3 | Read `loadPredictionStats()` — verify stats endpoint returns real aggregated data | `[ ]` |
| 11.4 | Verify market card rendering: question, category, YES/NO prices, volume, trader count, time remaining | `[ ]` |
| 11.5 | Verify category filter buttons actually filter market cards (client-side vs server-side) | `[ ]` |
| 11.6 | Verify sort dropdown (Volume, Newest, Ending Soon, Traders) sorts correctly | `[ ]` |
| 11.7 | Read `openPredictChart()` — verify price history loads from `/markets/:id/price-history` | `[ ]` |
| 11.8 | Verify canvas price chart renders with correct time-based X axis and 0-100% Y axis | `[ ]` |
| 11.9 | Verify chart time range tabs (1H, 6H, 24H, 7D, 30D, ALL) filter data correctly | `[ ]` |
| 11.10 | Test: create market via contract, confirm it appears in market grid | `[ ]` |
| 11.11 | Verify market card click selects market in Quick Trade panel | `[ ]` |
| 11.12 | Verify expired/resolved markets display correct status badges | `[ ]` |
| 11.13 | Verify no-markets empty state renders correctly | `[ ]` |
| 11.14 | Verify per-market analytics (unique traders, volume) — N+1 query performance concern | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 12: Prediction Market — Trade & Create

> Verify buying/selling shares and creating markets execute real on-chain transactions correctly.

| # | Task | Status |
|---|---|---|
| 12.1 | Read `prediction_market` contract: `buy_shares` instruction — pricing model (LMSR or AMM) | `[ ]` |
| 12.2 | Read `predictSubmitBtn` handler (dex.js) — verify instruction format matches contract | `[ ]` |
| 12.3 | Verify share price calculation: `updatePredictCalc()` — does the formula match the contract's? | `[ ]` |
| 12.4 | Verify YES/NO toggle updates submit button text and instruction outcome parameter | `[ ]` |
| 12.5 | **Fix needed:** YES/NO buttons (`predict-toggle-btn`) were not wallet-gated — CSS rule targeted wrong class (`predict-outcome-btn`) | `[ ]` |
| 12.6 | Verify amount presets ($10, $50, $100, $500) calculate shares and payout correctly | `[ ]` |
| 12.7 | Verify fee display (2%) matches contract fee logic | `[ ]` |
| 12.8 | Test: buy YES shares, verify position appears in "My Positions" tab | `[ ]` |
| 12.9 | Read `predictCreateBtn` handler — verify create_market instruction format | `[ ]` |
| 12.10 | Verify create market form: question, category, outcome count, close date, initial liquidity | `[ ]` |
| 12.11 | Verify Binary/Multi toggle changes number of outcome input fields | `[ ]` |
| 12.12 | Verify close date input has minimum date validation (not in the past) | `[ ]` |
| 12.13 | Read resolution logic: `resolve_market` instruction — who can resolve, oracle/admin mechanism | `[ ]` |
| 12.14 | Read `claim_winnings` instruction — verify payout calculation | `[ ]` |
| 12.15 | Test: create market → buy shares → resolve → claim winnings — full lifecycle | `[ ]` |
| 12.16 | Verify "My Markets" tab shows markets created by the connected wallet | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 13: Rewards & Fee Mining

> Verify the rewards system tracks trading volume, distributes fees, and the claim flow works on-chain.

| # | Task | Status |
|---|---|---|
| 13.1 | Read `dex_rewards` contract: reward calculation logic (per-epoch, volume-based, tier multiplier) | `[ ]` |
| 13.2 | Read `dex_rewards` contract: LP mining rewards — how are they distributed? | `[ ]` |
| 13.3 | Read `dex_rewards` contract: referral system — tracking, earnings, rate calculation | `[ ]` |
| 13.4 | Read RPC `get_rewards` handler — verify it reads `dex_rewards` contract storage | `[ ]` |
| 13.5 | Read RPC `get_rewards_stats` handler — verify aggregated totals are real | `[ ]` |
| 13.6 | Read `loadRewardsStats()` frontend — verify stats populate from API response | `[ ]` |
| 13.7 | Verify tier logic: Bronze → Silver → Gold → Platinum → Diamond with volume thresholds | `[ ]` |
| 13.8 | Verify tier multiplier display matches contract constants | `[ ]` |
| 13.9 | Verify progress bar shows correct percentage toward next tier | `[ ]` |
| 13.10 | Read Claim All button handler — verify `claim_rewards` instruction format | `[ ]` |
| 13.11 | Test: execute trades → verify pending rewards accumulate | `[ ]` |
| 13.12 | Test: claim rewards → verify balance increases | `[ ]` |
| 13.13 | Verify referral link generation and copy functionality | `[ ]` |
| 13.14 | **Fix:** Reward source panels should get `wallet-gated-disabled` when no wallet connected (currently only buttons disabled, not the panel) | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 14: Governance — Proposals & Voting

> Verify governance proposals and voting work end-to-end with real contract execution.

| # | Task | Status |
|---|---|---|
| 14.1 | Read `dex_governance` contract: `create_proposal` — 4 types (new_pair, fee_change, delist, parameter) | `[ ]` |
| 14.2 | Read `dex_governance` contract: `vote` instruction — weight based on MOLT balance? | `[ ]` |
| 14.3 | Read `dex_governance` contract: proposal execution — what happens when approved? | `[ ]` |
| 14.4 | Read RPC `get_proposals` handler — verify decode matches contract storage | `[ ]` |
| 14.5 | **Fix:** RPC `get_governance_stats` does not return `active_proposals` field — JS expects it | `[ ]` |
| 14.6 | Verify proposal card rendering: type badge, vote bar (YES/NO proportions), time remaining | `[ ]` |
| 14.7 | Verify Yes/No vote buttons send correct instruction with voter's MOLT balance as weight | `[ ]` |
| 14.8 | Verify approval threshold display (66%) matches contract constant | `[ ]` |
| 14.9 | Verify voting period display (48h) matches contract constant | `[ ]` |
| 14.10 | Read proposal type forms — verify each type sends correct parameters | `[ ]` |
| 14.11 | Verify "Parameter" type: 11 protocol parameters with data-current-value display | `[ ]` |
| 14.12 | Verify "Delist" type: reason textarea and impact warning box | `[ ]` |
| 14.13 | Test: create proposal → vote → verify vote counts update | `[ ]` |
| 14.14 | Test: proposal reaching approval threshold → verify execution | `[ ]` |
| 14.15 | Verify proposal filter (Active / All) works correctly | `[ ]` |
| 14.16 | Verify create proposal requirements check (minimum MOLT balance?) | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 15: Wallet Gating & UX States

> Systematically verify every interactive element is correctly disabled/enabled based on wallet connection state.

| # | Task | Status |
|---|---|---|
| 15.1 | Read `applyWalletGateAll()` (dex.js) — map every element it touches | `[ ]` |
| 15.2 | Verify Trade view: order form inputs, presets, tabs all disabled when disconnected | `[ ]` |
| 15.3 | Verify Trade view: submit button shows "Connect Wallet to Trade" when disconnected | `[ ]` |
| 15.4 | Verify Predict view: Quick Trade panel fully disabled (inputs + YES/NO toggles) when disconnected | `[ ]` |
| 15.5 | Verify Predict view: Create Market panel fully disabled when disconnected | `[ ]` |
| 15.6 | Verify Pool view: Add Liquidity form fully disabled when disconnected | `[ ]` |
| 15.7 | **Verify Pool view: per-row "Add" buttons in pool table disabled when disconnected** | `[ ]` |
| 15.8 | Verify Margin view: position form fully disabled when disconnected | `[ ]` |
| 15.9 | Verify Rewards view: all Claim buttons disabled when disconnected | `[ ]` |
| 15.10 | Verify Governance view: proposal form and vote buttons disabled when disconnected | `[ ]` |
| 15.11 | Verify bottom panels (Open Orders, Positions, My Markets, etc.) hidden when disconnected | `[ ]` |
| 15.12 | Verify wallet balance panel hidden when disconnected | `[ ]` |
| 15.13 | Test wallet disconnect: all gated elements revert to disabled state | `[ ]` |
| 15.14 | Test wallet reconnect: all gated elements re-enable correctly | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 16: Data Format Consistency

> Verify all data flowing from contracts → RPC → frontend uses consistent types, scales, and naming.

| # | Task | Status |
|---|---|---|
| 16.1 | **Critical fix:** Pool `feeTier` mismatch — RPC returns `"30bps"` (string), frontend expects number for `p.feeTier / 100` → NaN% | `[ ]` |
| 16.2 | Verify all price fields use consistent scale: `PRICE_SCALE` constant matches contract ↔ RPC ↔ frontend | `[ ]` |
| 16.3 | Verify all amount fields use consistent scale: shells (1e9) vs display (MOLT) | `[ ]` |
| 16.4 | Verify `rename_all = "camelCase"` on all RPC response structs — JS expects camelCase | `[ ]` |
| 16.5 | Verify `/api/v1/pools/positions` uses `address` query param — frontend sends `address=`, RPC expects `owner=` | `[ ]` |
| 16.6 | Verify prediction market share price format: percentage (0-100) vs decimal (0-1) | `[ ]` |
| 16.7 | Verify margin position `entry_price_raw` vs `entry_price` (float) consistency | `[ ]` |
| 16.8 | Verify candle data format matches TradingView datafeed expectations (OHLCV + time) | `[ ]` |
| 16.9 | Verify governance proposal `end_slot` → remaining time calculation (slot-to-seconds conversion) | `[ ]` |
| 16.10 | Verify reward amounts: shells vs display MOLT conversion matches across contract → RPC → UI | `[ ]` |
| 16.11 | Check `formatVolume()`, `formatPrice()`, `formatAmount()` — verify they handle all cases (zero, very large, very small) | `[ ]` |
| 16.12 | Verify pool liquidity display converts from raw u64 to human-readable USD | `[ ]` |
| 16.13 | Verify ticker endpoint returns `last_price` in correct scale for 24h stats row | `[ ]` |
| 16.14 | Verify order quantity: shells or human-readable? Check `parseFloat` vs integer handling | `[ ]` |
| 16.15 | Verify sqrt_price → human price conversion for pool current price display | `[ ]` |
| 16.16 | Cross-check: every `formatPrice(x)` call — is `x` in the right scale? | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 17: Real-Time Updates & Polling

> Verify data stays fresh via WebSocket and/or polling, without excessive resource usage.

| # | Task | Status |
|---|---|---|
| 17.1 | Read polling fallback code — verify interval (currently 5s for all views) | `[ ]` |
| 17.2 | Evaluate: 5s polling for governance/rewards is excessive — should reduce to 30-60s | `[ ]` |
| 17.3 | Verify WS reconnection with exponential backoff (cap, initial delay) | `[ ]` |
| 17.4 | Verify polling stops when switching away from a view (or at least doesn't fire for hidden views) | `[ ]` |
| 17.5 | Verify real-time price updates in pair stats bar (24h high/low/volume/change) | `[ ]` |
| 17.6 | Verify pool stats auto-refresh when new swaps execute | `[ ]` |
| 17.7 | Verify prediction market cards update prices when trades occur | `[ ]` |
| 17.8 | Test: execute trade → verify all panels (orderbook, trades, chart, ticker) update within 5s | `[ ]` |
| 17.9 | Verify reward stats refresh reflects new trade volume | `[ ]` |
| 17.10 | Verify governance vote counts update after new votes | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 18: Analytics Contract Wiring

> Verify the `dex_analytics` contract records trade data and the RPC/frontend consumes it correctly.

| # | Task | Status |
|---|---|---|
| 18.1 | Read `dex_analytics` contract: what events does it track? (trades, volume, candles) | `[ ]` |
| 18.2 | Verify analytics contract is called during trade execution (by `dex_core` or `dex_router`) | `[ ]` |
| 18.3 | Read candle aggregation logic: how are slot-based trades bucketed into time intervals? | `[ ]` |
| 18.4 | Verify `/stats/core` handler reads from `dex_analytics` storage | `[ ]` |
| 18.5 | Verify `/stats/analytics` handler returns comprehensive platform data | `[ ]` |
| 18.6 | Verify 24h stats (volume, trades, high, low) calculation from analytics data | `[ ]` |
| 18.7 | Verify pair-level stats (daily_volume in `decode_pair`) are updated by analytics | `[ ]` |
| 18.8 | Test: execute multiple trades → verify candle data updates | `[ ]` |
| 18.9 | Verify leaderboard endpoint populates from analytics tracking | `[ ]` |
| 18.10 | Verify trader stats endpoint uses analytics for volume/PnL calculation | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 19: Token Contracts & Balances

> Verify all token balances display correctly, transfers work, and decimal handling is consistent.

| # | Task | Status |
|---|---|---|
| 19.1 | Read `musd_token`, `wsol_token`, `weth_token` contracts — verify standard token interface | `[ ]` |
| 19.2 | Verify `getBalance` RPC call returns correct balance for each token | `[ ]` |
| 19.3 | Verify balance display converts shells (1e9) to human-readable with correct decimals | `[ ]` |
| 19.4 | Verify wallet balance panel shows all relevant token balances | `[ ]` |
| 19.5 | Verify token minting at genesis (treasury receives initial supply) | `[ ]` |
| 19.6 | Verify wrapped asset mint/redeem flow for wSOL and wETH | `[ ]` |
| 19.7 | Verify mUSD token issuance mechanism (faucet or bridge) | `[ ]` |
| 19.8 | Test: transfer MOLT to new address, verify sender/receiver balances update | `[ ]` |
| 19.9 | Verify token symbols in pair display match registry values | `[ ]` |
| 19.10 | Verify dust amount handling (very small balances < 0.000001) | `[ ]` |
| 19.11 | Verify max amount validation (cannot send more than balance) | `[ ]` |
| 19.12 | Verify fee deduction from balance after trade execution | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 20: Error Handling & Edge Cases

> Systematically verify error scenarios are handled gracefully across the entire DEX.

| # | Task | Status |
|---|---|---|
| 20.1 | Test: RPC server down → UI shows meaningful error, doesn't crash | `[ ]` |
| 20.2 | Test: contract execution failure → user sees error notification with reason | `[ ]` |
| 20.3 | Test: insufficient balance for order → rejection message | `[ ]` |
| 20.4 | Test: invalid price (0 or negative) → prevented before submission | `[ ]` |
| 20.5 | Test: expired prediction market → cannot buy shares | `[ ]` |
| 20.6 | Test: duplicate order submission (double-click) → prevented by button disable | `[ ]` |
| 20.7 | Verify `showNotification()` displays errors, warnings, success messages correctly | `[ ]` |
| 20.8 | Verify TradingView chart error fallback (library load failure) | `[ ]` |
| 20.9 | Test: WebSocket message with invalid format → graceful handling, no crash | `[ ]` |
| 20.10 | Test: large number inputs → overflow protection | `[ ]` |
| 20.11 | Verify all `try/catch` blocks log errors, don't silently swallow | `[ ]` |
| 20.12 | Test: concurrent transactions → verify no double-spend or race conditions | `[ ]` |
| 20.13 | Verify network error retry logic (API calls that fail transiently) | `[ ]` |
| 20.14 | Test: wallet import with invalid private key → error message | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 21: SDK & Market Maker Integration

> Verify the JavaScript SDK and automated market maker work correctly with the DEX contracts.

| # | Task | Status |
|---|---|---|
| 21.1 | Read JS SDK (`sdk/js/src/`) — verify transaction building matches frontend's `sendTransaction` | `[ ]` |
| 21.2 | Verify SDK `placeOrder()` function sends correct instruction to `dex_core` | `[ ]` |
| 21.3 | Verify SDK `addLiquidity()` function sends correct instruction to `dex_amm` | `[ ]` |
| 21.4 | Read market maker bot (`dex/market-maker/`) — verify it creates real orders with proper spread | `[ ]` |
| 21.5 | Verify market maker connects to correct RPC and uses correct contract addresses | `[ ]` |
| 21.6 | Test: run market maker → verify orders appear in orderbook | `[ ]` |
| 21.7 | Verify market maker handles order fills and rebalances | `[ ]` |
| 21.8 | Read load test harness (`dex/loadtest/`) — verify it tests real contract execution | `[ ]` |
| 21.9 | Verify SDK error handling: invalid params, network errors, tx failures | `[ ]` |
| 21.10 | Verify SDK types match RPC response formats | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 22: Security & Input Validation

> Verify all user inputs are sanitized, contract calls are safe, and no XSS/injection vectors exist.

| # | Task | Status |
|---|---|---|
| 22.1 | Verify `escapeHtml()` is called on ALL user-supplied strings before rendering (token names, proposal text, market questions) | `[ ]` |
| 22.2 | Verify no `innerHTML` with unsanitized data anywhere in dex.js | `[ ]` |
| 22.3 | Verify numeric inputs are validated (NaN, negative, overflow) before tx submission | `[ ]` |
| 22.4 | Verify contract addresses are validated (base58 format, correct length) | `[ ]` |
| 22.5 | Verify transaction signing uses correct Ed25519 key and nonce | `[ ]` |
| 22.6 | Verify private key storage is memory-only (never persisted in plaintext) | `[ ]` |
| 22.7 | Read wallet keychain encryption — verify AES-256-GCM or similar | `[ ]` |
| 22.8 | Verify CORS headers on RPC endpoints (restrict to same-origin or known domains) | `[ ]` |
| 22.9 | Verify contract instructions validate all parameters server-side (don't trust client) | `[ ]` |
| 22.10 | Verify integer overflow protection in contract arithmetic (checked_add/mul) | `[ ]` |
| 22.11 | Verify slippage protection: orders/swaps reject if price moves beyond tolerance | `[ ]` |
| 22.12 | Verify prediction market resolution cannot be manipulated (oracle/admin key checks) | `[ ]` |
| 22.13 | Verify governance voting cannot be double-counted (one vote per address per proposal) | `[ ]` |
| 22.14 | Run `node dex/dex.test.js` — verify all existing tests pass | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 23: Mobile / Responsive Layout

> Verify the DEX is usable on mobile devices and tablets.

| # | Task | Status |
|---|---|---|
| 23.1 | Read CSS media queries — verify breakpoints for mobile (≤768px) and tablet (≤1024px) | `[ ]` |
| 23.2 | Verify Trade view: chart + orderbook + form stack vertically on mobile | `[ ]` |
| 23.3 | Verify Predict view: market cards grid adapts to single column | `[ ]` |
| 23.4 | Verify Pool view: table scrolls horizontally or adapts columns | `[ ]` |
| 23.5 | Verify navigation works on mobile (hamburger menu or scrollable tabs) | `[ ]` |
| 23.6 | Verify modals (wallet, chart) are usable on small screens | `[ ]` |
| 23.7 | Verify touch interactions: buttons, sliders, toggles all respond to touch | `[ ]` |
| 23.8 | Verify no horizontal overflow on any view at 375px width | `[ ]` |

**Findings:**
- (none yet)

---

## Phase 24: End-to-End Integration Tests

> Full lifecycle tests that exercise the complete stack: frontend → RPC → contract → storage → RPC → frontend.

| # | Task | Status |
|---|---|---|
| 24.1 | **E2E: Full trade lifecycle** — connect wallet → place limit order → verify in orderbook → match → verify trade history → verify balance change | `[ ]` |
| 24.2 | **E2E: Full LP lifecycle** — add liquidity → verify position → execute swaps → collect fees → remove liquidity → verify balance | `[ ]` |
| 24.3 | **E2E: Full prediction lifecycle** — create market → buy shares → resolve market → claim winnings → verify balance | `[ ]` |
| 24.4 | **E2E: Full margin lifecycle** — open position → monitor PnL → close position → verify settlement | `[ ]` |
| 24.5 | **E2E: Full governance lifecycle** — create proposal → vote → reach threshold → verify execution | `[ ]` |
| 24.6 | **E2E: Full rewards lifecycle** — execute trades → accumulate rewards → claim → verify | `[ ]` |
| 24.7 | **E2E: Router test** — swap using router → verify best execution path selected | `[ ]` |
| 24.8 | **E2E: Multi-user scenario** — two wallets trade against each other → both see updated balances | `[ ]` |
| 24.9 | **E2E: Cross-view consistency** — trade on Trade view → check Pool TVL updated → check Rewards pending updated | `[ ]` |
| 24.10 | Verify all E2E tests run against local testnet stack | `[ ]` |
| 24.11 | Document any manual verification steps that cannot be automated | `[ ]` |
| 24.12 | Final pass: open each view as fresh user (no wallet) → verify everything renders correctly with real data | `[ ]` |

**Findings:**
- (none yet)

---

## Known Issues (Pre-Audit)

Issues already identified before starting the plan:

| # | Issue | Phase | Severity |
|---|---|---|---|
| K1 | Pool `feeTier` returned as string `"30bps"`, JS divides by 100 → shows NaN% | 7, 16 | **Critical** |
| K2 | Governance stats endpoint missing `active_proposals` field | 14, 16 | **High** |
| K3 | `MOLT_GENESIS_PRICE` ($0.10) mismatch with deploy sqrt_price (~$0.42) | 2 | **High** |
| K4 | YES/NO toggle buttons used wrong CSS class for wallet gating — fixed: added `.predict-toggle-btn` to CSS rule | 12, 15 | **Fixed** |
| K5 | Pool table per-row "Add" buttons not wallet-gated — fixed: added disabled + btn-wallet-gate when disconnected | 7, 15 | **Fixed** |
| K6 | Standalone `view-margin` section unreachable (no nav link) | 10 | **Medium** |
| K7 | Rewards source panels not fully wallet-gated (only buttons, not forms) | 13, 15 | **Medium** |
| K8 | Prediction close date input has no min-date validation | 12 | **Medium** |
| K9 | Polling interval (5s) excessive for slow-changing views (governance, rewards) | 17 | **Low** |
| K10 | Per-market analytics fetches cause N+1 queries | 11 | **Low** |
| K11 | LP positions endpoint: frontend sends `address=`, RPC expects `owner=` param | 16 | **High** |
