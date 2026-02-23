# MoltChain Production Gaps Tracker

> Last updated: 2026-02-18
> Tracking all remaining gaps from the 3 production audits.

---

## 1. Contract Upgrade System

| # | Gap | File(s) | Status |
|---|-----|---------|--------|
| U1 | CLI `molt upgrade` command | cli/src/main.rs, cli/src/client.rs | ✅ Done |
| U2 | RPC `upgradeContract` endpoint | rpc/src/lib.rs | ✅ Done |
| U3 | `ProgramDeployer.upgrade()` method | programs/js/moltchain-sdk.js | ✅ Done |
| U4 | Playground upgrade button handler | programs/js/playground-complete.js | ✅ Done |
| U5 | Version field in ContractAccount | core/src/contract.rs | ✅ Done |
| U6 | Version tracking on upgrade (bump + history) | core/src/processor.rs | ✅ Done |
| U7 | Unit tests for contract_upgrade (3 tests) | core/tests/contract_lifecycle.rs | ✅ Done |

---

## 2. DEX WebSocket — Event Emission (Producer Side)

P0-A wired the subscriber side (`subscribeDex`). The producer side now emits
`DexEvent` into `DexEventBroadcaster` when DEX state changes occur.

| # | Feature | REST (working) | WS Channel | Emit Site | Status |
|---|---------|---------------|------------|-----------|--------|
| D1 | Order book updates | `POST /orders` | `orderbook:<pair_id>` | post_order, delete_order | ✅ Done |
| D2 | Trade stream | `POST /router/swap` | `trades:<pair_id>` | post_router_swap | ✅ Done |
| D3 | Price ticker | `POST /router/swap` | `ticker:<pair_id>` | post_router_swap | ✅ Done |
| D4 | OHLCV candles | (emit placeholder) | `candles:<pair_id>:<interval>` | — | ✅ Wired (candle aggregation deferred) |
| D5 | User orders | `POST /orders`, `DELETE /orders/:id` | `orders:<trader_addr>` | post_order, delete_order | ✅ Done |
| D6 | Margin positions | `POST /margin/open`, `POST /margin/close` | `positions:<trader_addr>` | post_margin_open, post_margin_close | ✅ Done |

**Infra**: `dex_broadcaster: Arc<DexEventBroadcaster>` added to RpcState (rpc/src/lib.rs).

---

## 3. Prediction Market WebSocket

| # | Gap | Status |
|---|-----|--------|
| P1 | Define `subscribePredictionMarket` subscription type + PredictionChannel + PredictionEvent | ✅ Done |
| P2 | Wire handler in ws.rs (subscribe/unsubscribe + forwarding task) | ✅ Done |
| P3 | Emit events from prediction market REST handlers (post_trade, post_create) | ✅ Done |

**Infra**: `prediction_broadcaster: Arc<PredictionEventBroadcaster>` added to RpcState and WsState. Validator start_ws_server call updated.

---

## 4. Already Completed (P0 commit b471b47)

| # | Fix | File(s) | Status |
|---|-----|---------|--------|
| P0-A | `subscribeDex`/`unsubscribeDex` in WS handler | rpc/src/ws.rs, validator/src/main.rs | ✅ Done |
| P0-B | Deposit status lifecycle (swept/credited) | custody/src/main.rs | ✅ Done |
| P0-C | Bridge WS subscriptions in wallet | wallet/js/wallet.js | ✅ Done |
| P0-D | Extension home.js XSS fix | wallet/extension/src/pages/home.js | ✅ Done |

---

## Build & Validation

| Step | Status |
|------|--------|
| `cargo check` all modified crates | ✅ Pass |
| `cargo test` contract_lifecycle (8/8 pass) | ✅ Pass |
| `cargo build --release` | ⬜ Not run |
| 3-validator testnet boot | ⬜ Not run |
| E2E tests pass | ⬜ Not run |
| Final commit | ⬜ Pending |
