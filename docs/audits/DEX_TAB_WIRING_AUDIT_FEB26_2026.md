# DEX Tab Wiring Audit (Production Readiness)

Date: 2026-02-26  
Scope: `dex/index.html`, `dex/dex.js`, `rpc/src/dex.rs`, `rpc/src/prediction.rs`, `rpc/src/launchpad.rs`, `rpc/src/ws.rs`, `rpc/src/dex_ws.rs`

## Executive Summary

- Core tab wiring is present across Trade, Predict, Pool, Rewards, Governance, and Launchpad.
- Contract write-path wiring exists for all primary actions through `sendTransaction` contract calls.
- Real-time WS wiring exists for both DEX market channels and prediction channels.
- Launch-critical gaps remain in API parity and one dead code branch:
  1. Missing REST config endpoints used by frontend (`/launchpad/config`, `/prediction-market/config`).
  2. Router swap execute endpoint exists (`/router/swap`) but is not wired to any user action.
  3. Unreachable poll branch for `currentView === 'margin'` (no margin view exists; margin is a mode inside Trade).

## Tab-by-Tab Wiring Matrix

## 1) Trade

UI
- Main view: `#view-trade`
- Key controls: pair selector, order form (spot/margin), open orders/history/positions

Frontend loaders
- `loadPairs`, `loadOrderBook`, `loadRecentTrades`, `loadCandles`, `loadTicker`
- `loadBalances`, `loadUserOrders`, `loadTradeHistory`, `loadMarginStats`, `loadMarginPositions`, `loadMarginHistory`

REST reads
- `/pairs`, `/pairs/:id/orderbook`, `/pairs/:id/trades`, `/pairs/:id/candles`, `/pairs/:id/ticker`
- `/orders?trader=...`
- `/margin/positions`, `/margin/info`, `/margin/enabled-pairs`, `/margin/funding-rate`
- `/stats/margin`, `/stats/amm`

WS realtime
- `subscribeDex` channels: `orderbook:<pair_id>`, `trades:<pair_id>`, `ticker:<pair_id>`, `orders:<address>`

Contract writes
- DEX core: place/modify/cancel/cancel-all order opcodes
- DEX margin: open/close/partial close/add-remove margin/set SL-TP opcodes

Status
- ✅ Wired end-to-end for primary CLOB + margin flows.

## 2) Predict

UI
- Main view: `#view-predict`
- Controls: market list, YES/NO quick trade, create market, positions/history/created tabs

Frontend loaders
- `loadPredictionStats`, `loadPredictionMarkets`, `loadPredictionPositions`, `loadPredictionHistory`, `loadCreatedMarkets`

REST reads
- `/prediction-market/stats`
- `/prediction-market/markets`, `/prediction-market/markets/:id`, `/prediction-market/markets/:id/analytics`, `/prediction-market/markets/:id/price-history`
- `/prediction-market/positions`, `/prediction-market/trades`, `/prediction-market/traders/:addr/stats`

WS realtime
- `subscribePrediction('all')` via WS methods `subscribePrediction` / `unsubscribePrediction`

Contract writes
- Prediction contract: create market, add initial liquidity, buy shares, redeem shares, resolve, challenge, finalize

Status
- ✅ Wired for read + write + realtime.
- ⚠ Frontend requests `/prediction-market/config` in protocol-params bootstrap, but no matching RPC route exists.

## 3) Pool

UI
- Main view: `#view-pool`
- Controls: pool table, add-liquidity form, LP positions/history panel

Frontend loaders
- `loadPoolStats`, `loadPools`, `loadLPPositions`

REST reads
- `/stats/amm`, `/pools`, `/pools/positions?owner=...`

Contract writes
- DEX AMM: add liquidity, remove liquidity, collect fees opcodes

Status
- ✅ Core LP operations wired.

## 4) Rewards

UI
- Main view: `#view-rewards`
- Controls: pending rewards, claim all/per-source claims, referral links, tier details

Frontend loaders
- `loadRewardsStats`, `loadRewards`, rewards rendering helpers

REST reads
- `/stats/rewards`, `/rewards/:addr`

Contract writes
- DEX rewards: claim rewards opcode

Status
- ✅ Read + claim pathways wired.

## 5) Governance

UI
- Main view: `#view-governance`
- Controls: proposal list, vote/finalize/execute actions, create-proposal form

Frontend loaders
- `loadGovernanceStats`, `loadProposals`

REST reads
- `/stats/governance`, `/governance/proposals`

Contract/API writes
- Vote: governance contract opcode path
- Finalize/execute: governance contract opcode path
- Create proposal: frontend uses on-chain proposal transaction flow (not REST create)

Status
- ✅ Primary governance lifecycle wired.

## 6) Launchpad

UI
- Main view: `#view-launchpad`
- Controls: token list + sorting/filter, bonding-curve panel, buy/sell form, create token

Frontend loaders
- `loadLaunchpadStats`, `loadLaunchpadTokens`, token selection/render/quote helpers

REST reads
- `/launchpad/stats`, `/launchpad/tokens`, `/launchpad/tokens/:id`, `/launchpad/tokens/:id/holders`

Contract writes
- ClawPump named calls: `create_token`, `buy`, `sell`

Status
- ✅ Primary launch/trade flows wired.
- ⚠ Frontend requests `/launchpad/config` in protocol-params bootstrap, but no matching RPC route exists.

## Missing / Suspicious Wiring (Action List)

1. Missing config route parity
- Frontend calls:
  - `GET /api/v1/launchpad/config`
  - `GET /api/v1/prediction-market/config`
- Backend route tables do not expose either endpoint.
- Current behavior is graceful fallback due `try/catch`, but this is still wiring debt and hides config drift.

2. Router execute endpoint not surfaced in UI
- Backend has `POST /api/v1/router/swap`.
- Frontend only calls `/router/quote` for estimation.
- No UI action currently executes routed swaps (only order-book style order placement).

3. Dead refresh branch
- Poller checks `state.currentView === 'margin'`.
- No standalone margin view exists; margin is a mode inside Trade.
- Branch is unreachable and can mislead future maintenance.

## Recommended Immediate Follow-ups

1. Add config endpoints
- Add `GET /launchpad/config` and `GET /prediction-market/config` with authoritative values from storage/contract state.

2. Decide router UX direction
- Either wire a dedicated “Swap” execution action to `/router/swap` or remove/feature-flag the endpoint from production UI expectations.

3. Remove dead margin-view poll code
- Replace with Trade-mode checks (`currentView === 'trade' && tradeMode === 'margin'`) or remove branch.

## Notes on Wallet-Gate Consistency

- Prediction YES/NO buttons are now wallet-gated with explicit disabled state and consistent disabled styling when disconnected or not signing-ready.
- This aligns Predict controls with the same UX pattern used by other gated actions.

## Wallet Switch/Signer Hotfix (Feb 26)

- Added persistent local signer session storage for imported/generated wallets (`dexWalletSessionsV1`) so wallet switching and page reload keep signing capability.
- Added switch-time extension signer hydration attempt for saved addresses without local signer state.
- Improved wallet-gated disabled button contrast so reconnect CTA text remains readable.

Files updated:
- `moltchain/dex/dex.js` (wallet session restore/persist, switch signer hydration)
- `moltchain/dex/dex.css` (wallet-gate disabled readability)

## Expanded Realtime + Matrix Test Plan (Binary + Multi-Outcome)

Predict lifecycle matrix:
1. Binary market buy path (`YES` and `NO`) updates market card, side panel quote, positions table, and PnL within one refresh cycle.
2. Multi-outcome market create path validates 3/4/8 outcomes, confirms UI renders all outcomes, and verifies correct outcome index mapping in tx payload.
3. Expiry transition test verifies market auto-shifts from active to closed/resolving state when slot/time boundary passes (no manual refresh).
4. Resolution lifecycle verifies resolve → challenge → finalize transitions and action-button gating updates in all predict sub-tabs.
5. Claim path verifies winner receives claim action and loser receives non-claim state, then post-claim position refresh reflects redeemed shares.

Realtime resilience matrix:
1. WS active: ticker/orderbook/trade/predict channels update in near real-time.
2. WS degraded: polling fallback converges state in <=10s for trade and <=5s for predict.
3. Tab switching stress: switching `trade/predict/pool/rewards/governance/launchpad` does not leave stale timers/subscriptions.

Cross-tab contract truth matrix:
1. Trade submit/cancel/modify reflected in open orders and history.
2. Pool add/remove/collect reflected in LP positions and pool TVL.
3. Rewards claim reflected in pending and all-time totals.
4. Governance vote/finalize/execute reflected in proposal cards and stats.
5. Launchpad buy/sell/create reflected in token cards, holdings, and launch stats.
