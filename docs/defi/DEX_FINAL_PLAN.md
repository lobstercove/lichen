# DEX Final Production Plan — Complete Feature Coverage

> Generated: Feb 19, 2026
> Mentality: Full implementation, no stubs, no placeholders, no mock data, fully wired and interacting with contracts.

---

## Current State Snapshot

### What's Solid (Production-Grade)
- Full CLOB matching engine with maker rebates, reentrancy guards, self-trade prevention
- 10 smart contracts deployed: dex_core, dex_margin, dex_amm, dex_router, dex_rewards, dex_governance, dex_analytics, prediction_market, clawpump, moltcoin
- 6-view frontend: Trade, Predict, Pool, Launch, Rewards, Governance
- TradingView chart integration with real datafeed + real-time bar updates
- WebSocket real-time: orderbook, trades, tickers, candles, order fills
- 55+ REST API endpoints
- Mobile-responsive CSS with hamburger navigation (5 breakpoints)
- Ed25519 wallet: import/create/switch/disconnect + transaction signing
- Fee estimation from router quote API
- Oracle reference price feed (Binance WS overlay)
- Volume simulation: 114 tests passing, DEX 84/84, Launchpad 41/41

### Bottom Panel (Trade View) — Current Layout
```
[ Open Orders ] [ Trade History ] [ Positions ] [ Margin ]
```
- **Open Orders**: Full table (Pair/Side/Type/Price/Amount/Filled/Time/Cancel)
- **Trade History**: Per-pair trade history for connected wallet
- **Positions**: Fetches margin positions — duplicate of Margin tab with LESS info
- **Margin**: Margin positions + equity/used/available stats strip

**Problems:**
- "Positions" and "Margin" tabs are redundant — both fetch `/margin/positions`
- No liquidation price on position rows
- No PnL % display (only absolute)
- No funding rate display
- No Add/Remove margin buttons (contract supports opcodes 4/5)

### Order Types — Current State
| Type | Contract Constant | Frontend UI | Actually Works |
|------|------------------|-------------|----------------|
| Limit | `ORDER_LIMIT=0` | ✅ Button | ✅ **Yes** |
| Market | `ORDER_MARKET=1` | ✅ Button | ✅ **Yes** |
| Stop-Limit | `ORDER_STOP_LIMIT=2` | ✅ Button + price field | ❌ **No** — stop price not encoded in TX, no trigger engine |
| Post-Only | `ORDER_POST_ONLY=3` | ✅ Checkbox in HTML | ❌ **No** — checkbox not wired to `buildPlaceOrderArgs` |
| Reduce-Only | N/A | ✅ Checkbox in HTML | ❌ **No** — zero JS logic |

### Contract Capabilities NOT Exposed in Frontend
| Contract | Function | Opcode | Frontend |
|----------|----------|--------|----------|
| dex_core | `modify_order` | 16 | ❌ No UI |
| dex_core | `cancel_all_orders` | 17 | ❌ No UI |
| dex_margin | `add_margin` | 4 | ❌ No UI |
| dex_margin | `remove_margin` | 5 | ❌ No UI |
| dex_governance | `finalize_proposal` | exists | ❌ No UI |
| dex_governance | `execute_proposal` | exists | ❌ No UI |
| prediction_market | `challenge_resolution` | exists | ❌ No UI |
| prediction_market | `mint_complete_set` | exists | ❌ No UI |
| prediction_market | `redeem_complete_set` | exists | ❌ No UI |

---

## Phase 1: Bottom Panel Consolidation — 5 Tasks

Merge the redundant "Positions" and "Margin" tabs into one unified, information-rich panel.

### Task 1.1: Remove duplicate Positions tab
- **Layer:** HTML (`dex/index.html`)
- **What:** Remove the `content-positions` tab button and its content div. It's a duplicate of Margin with less info.
- **Detail:** Remove `<button class="pos-tab" data-target="content-positions">Positions</button>` and `<div id="content-positions">` section.

### Task 1.2: Rename Margin tab → Positions
- **Layer:** HTML (`dex/index.html`)
- **What:** Rename the remaining "Margin" tab to "Positions" since these are the only positions we carry (CLOB fills are instant, not carried as spot positions).
- **Detail:** Change tab text from "Margin" to "Positions". Update `data-target` from `content-margin-positions` to `content-positions`.

### Task 1.3: Add liquidation price to position rows
- **Layer:** JS (`dex/dex.js`)
- **What:** Compute and display liquidation price on each margin position row.
- **Formula:**
  - Long: `liqPrice = entryPrice * (1 - margin / (size * entryPrice) + maintenanceMarginBps / 10000)`
  - Short: `liqPrice = entryPrice * (1 + margin / (size * entryPrice) - maintenanceMarginBps / 10000)`
- **Detail:** Add "Liq. Price" column to both `loadMarginPositions()` and `loadPositionsTab()` renders. Use tier maintenance margin BPS from contract constants.

### Task 1.4: Add PnL % display
- **Layer:** JS (`dex/dex.js`)
- **What:** Show PnL as percentage alongside absolute value.
- **Formula:** `pnlPct = (pnl / margin) * 100`
- **Detail:** Update position row HTML to show `+12.5% (+0.0125 MOLT)` format. Green for profit, red for loss.

### Task 1.5: Add margin management buttons (Add/Remove Margin)
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** Add "＋" and "−" buttons on each position row to add or remove margin.
- **Contract:** `dex_margin` opcode 4 (`add_margin(caller, position_id, amount)`) and opcode 5 (`remove_margin(caller, position_id, amount)`)
- **Detail:**
  - Small inline input appears on click (amount field + confirm button)
  - New builders: `buildAddMarginArgs(trader, positionId, amount)` and `buildRemoveMarginArgs(trader, positionId, amount)`
  - Both use 49-byte layout: opcode(1) + trader(32) + position_id(8) + amount(8)
  - Success → refresh positions, refresh balances
  - Wallet gate: disabled when not connected

---

## Phase 2: Stop-Loss / Take-Profit System — 8 Tasks

Full conditional order system — both as standalone entry orders and exit triggers attached to margin positions.

### Task 2.1: Extend `buildPlaceOrderArgs` to encode stop price
- **Layer:** JS (`dex/dex.js`)
- **What:** Add `stopPrice` parameter. New total: 75 bytes (was 67).
- **Layout:** Existing 67 bytes + `stopPrice` at offset 67 (8 bytes LE u64)
- **Detail:** When `orderType === 'stop-limit'`, set type byte to `2` and encode the stop/trigger price. When `orderType !== 'stop-limit'`, write 0.

### Task 2.2: Extend `place_order` contract to store trigger price
- **Layer:** Contract (`contracts/dex_core/src/lib.rs`)
- **What:** Extend order storage layout by 8 bytes for `trigger_price`. Stop-limit orders stored as dormant (status = `STATUS_DORMANT=5`), not placed in the active book.
- **Detail:**
  - Parse `trigger_price` from args bytes 67..75
  - If `order_type == ORDER_STOP_LIMIT && trigger_price > 0`: store order with `STATUS_DORMANT` instead of `STATUS_OPEN`
  - Dormant orders are NOT matched during `place_order` — they wait for trigger
  - Add `STATUS_DORMANT: u8 = 5` constant

### Task 2.3: Add `check_triggers` contract function
- **Layer:** Contract (`contracts/dex_core/src/lib.rs`)
- **What:** New opcode (27): scans dormant stop-limit orders for a pair, compares trigger price to last trade price, converts triggered orders to active limit orders.
- **Args:** `(pair_id: u64, last_price: u64)` — 16 bytes
- **Logic:**
  - Iterate dormant orders for the pair
  - For sell-stops: if `last_price <= trigger_price`, activate (set status=OPEN, attempt match)
  - For buy-stops: if `last_price >= trigger_price`, activate
  - Return count of triggered orders
- **Caller:** Validator or keeper bot (permissionless — anyone can call)

### Task 2.4: Wire stop price field in order form
- **Layer:** JS (`dex/dex.js`)
- **What:** Read `#stopPrice` input value and pass to `buildPlaceOrderArgs`.
- **Validation:**
  - Sell-stop: stop price must be BELOW current market price
  - Buy-stop: stop price must be ABOVE current market price
  - Show validation error message if invalid
- **Detail:** Update the submit handler section to read `document.getElementById('stopPrice').value` when `state.orderType === 'stop-limit'`.

### Task 2.5: Add TP/SL inputs on margin position open
- **Layer:** HTML + JS (`dex/index.html`, `dex/dex.js`)
- **What:** When the margin toggle is active in the trade form, show optional "Stop Loss" and "Take Profit" price fields below the leverage slider.
- **Flow:** On margin position open → if SL/TP filled → create linked stop-limit orders targeting the position
- **Detail:** After successful `open_position`, automatically call `place_order` with `ORDER_STOP_LIMIT` for each SL/TP price set.

### Task 2.6: Add `set_position_sl_tp` margin contract function
- **Layer:** Contract (`contracts/dex_margin/src/lib.rs`)
- **What:** New opcode: stores SL and TP prices on an existing position. When triggered, auto-closes position.
- **Args:** `(caller[32], position_id[8], stop_loss_price[8], take_profit_price[8])` — 56 bytes
- **Storage:** Add `sl_price` (8 bytes) and `tp_price` (8 bytes) to position layout
- **Detail:** These prices are checked by a keeper/validator. When mark price crosses SL or TP, `close_position` is auto-called.

### Task 2.7: Add TP/SL edit on existing positions
- **Layer:** JS (`dex/dex.js`)
- **What:** "SL/TP" button on each position row → small modal/inline panel with SL price and TP price inputs.
- **On confirm:** Call `set_position_sl_tp` via `sendTransaction`
- **Display:** Show SL/TP values on position row if set (small red/green badges)

### Task 2.8: Add trigger engine to validator tick
- **Layer:** Rust (`validator/`)
- **What:** Each block, call `check_triggers` for active pairs that had trades. Also check margin position SL/TP.
- **Detail:**
  - After processing a block with trades, iterate affected pairs
  - Call `dex_core::check_triggers(pair_id, last_price)` for each
  - Call `dex_margin` SL/TP check for positions on affected pairs
  - Low cost — only scans dormant orders / positions with SL/TP set

---

## Phase 3: Order Form Completeness — 5 Tasks

Wire the existing but non-functional UI controls and add safety features.

### Task 3.1: Wire Post-Only checkbox
- **Layer:** JS (`dex/dex.js`)
- **What:** Read `#postOnly` checkbox state. If checked, set `orderType` byte to `3` (`ORDER_POST_ONLY`) in `buildPlaceOrderArgs`.
- **Contract behavior:** Post-only orders that would immediately match against the book are REJECTED (return error code). They can only add liquidity (sit on the book as maker).
- **Detail:** Already `ORDER_POST_ONLY=3` in contract. Only needs JS wiring: `if (postOnly.checked) orderTypeByte = 3;`

### Task 3.2: Wire Reduce-Only checkbox
- **Layer:** JS (`dex/dex.js`)
- **What:** Reduce-only ensures order only reduces an existing position — never opens a new one or increases size.
- **MVP approach (client-side validation):**
  - Only relevant when margin is active
  - Check: if selling, user must have an open Long position on this pair with size ≥ order amount
  - Check: if buying, user must have an open Short position on this pair with size ≥ order amount
  - If validation fails, show notification and block submission
- **Future:** Contract-level enforcement via new order flag

### Task 3.3: Add "Cancel All Orders" button
- **Layer:** HTML + JS (`dex/index.html`, `dex/dex.js`)
- **What:** Add "Cancel All" button in the Open Orders tab header, next to the tab title.
- **Contract:** `dex_core` opcode 17 — `cancel_all_orders(caller[32], pair_id[8])` — 41 bytes
- **New builder:** `buildCancelAllOrdersArgs(trader, pairId)`
- **Confirmation:** Show "Cancel all orders on {PAIR}?" dialog before executing

### Task 3.4: Add order modification (edit in-place)
- **Layer:** JS (`dex/dex.js`)
- **What:** "Edit" button on each open order row → inline editable price/qty fields → "Save" button
- **Contract:** `dex_core` opcode 16 — `modify_order(caller[32], order_id[8], new_price[8], new_qty[8])` — 57 bytes
- **New builder:** `buildModifyOrderArgs(trader, orderId, newPrice, newQty)`
- **Detail:** Modification is internally cancel + re-place at new params. Order ID stays the same.

### Task 3.5: Order confirmation dialog
- **Layer:** JS (`dex/dex.js`, `dex/index.html`)
- **What:** Before submitting any margin trade or spot orders > $100 equivalent, show confirmation modal.
- **Shows:** Order Type, Side, Price, Amount, Total, Est. Fee, Leverage (if margin)
- **User option:** "Don't show again for small orders" checkbox → stored in `localStorage`
- **Skip:** Market orders under $100 on spot (fast execution expected)

---

## Phase 4: Margin Position Enhancements — 4 Tasks

### Task 4.1: Funding rate display
- **Layer:** JS + API (`dex/dex.js`, `rpc/src/dex.rs`)
- **What:** Show current funding rate in margin stats strip.
- **API:** New endpoint `/api/v1/margin/funding-rate` — returns base rate from contract constants per tier
- **Display:** In the margin equity stats strip: "Funding: 0.01%/8h" styled badge
- **Note:** `process_funding()` isn't implemented in contract yet — display the configured constant rate. When contract implements it, the display is already wired.

### Task 4.2: Partial position close
- **Layer:** JS + Contract (`dex/dex.js`, `contracts/dex_margin/src/lib.rs`)
- **What:** "Partial Close" option on positions — specify percentage (25/50/75%) or custom amount.
- **Contract:** New opcode — `partial_close(caller[32], position_id[8], close_amount[8])` — 49 bytes
  - Splits position: reduces size by `close_amount`, settles proportional PnL, adjusts margin proportionally
  - Creates a new position record for the remainder, or modifies in-place
- **Frontend:** Dropdown on Close button: "Close 100%" / "Close 25%" / "Close 50%" / "Close 75%" / "Custom"

### Task 4.3: Position PnL share card
- **Layer:** JS (`dex/dex.js`)
- **What:** "Share" button on position rows → generates a styled PnL card using Canvas.
- **Card shows:** Pair, Side, Entry, Mark, PnL $, PnL %, Leverage, Duration
- **Actions:** "Copy Image" (to clipboard) or "Download PNG"
- **Styling:** Branded with MoltChain gradient background, green for profit / red for loss

### Task 4.4: Cross-margin mode (design doc only)
- **Layer:** Documentation
- **What:** Write a design document for cross-margin where all positions share a single margin pool.
- **Rationale:** Isolated margin is the production MVP. Cross-margin is a future enhancement.
- **Defer:** No implementation — only design spec saved for reference.

---

## Phase 5: Settings & Preferences — 4 Tasks

### Task 5.1: User-adjustable slippage tolerance
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** Settings gear icon in order form area → popover panel with slippage options.
- **Options:** 0.1% / 0.5% (default) / 1.0% / Custom input
- **Storage:** `localStorage.setItem('dexSlippage', value)`
- **Wired to:** Router quote API call and swap `min_out` calculation
- **Currently:** Hardcoded 0.5% in router quote

### Task 5.2: Notification preferences
- **Layer:** JS + HTML
- **What:** In settings popover: toggle switches for:
  - Trade fill notifications (on/off)
  - Order partial fill notifications (on/off)
  - Liquidation proximity warning (on/off) — flash position row when margin ratio < 120%
- **Storage:** `localStorage.setItem('dexNotifPrefs', JSON.stringify(prefs))`

### Task 5.3: Chart interval memory
- **Layer:** JS (`dex/dex.js`)
- **What:** Remember last-used chart interval in `localStorage`.
- **Currently:** Always starts at `'15'` (15-minute candles)
- **Fix:** On interval change → `localStorage.setItem('dexChartInterval', interval)`. On init → `localStorage.getItem('dexChartInterval') || '15'`

### Task 5.4: Default pair memory
- **Layer:** JS (`dex/dex.js`)
- **What:** Remember last active pair in `localStorage`. Auto-select on page load.
- **Currently:** Always selects first pair
- **Fix:** On pair switch → `localStorage.setItem('dexLastPair', pairId)`. On init → find saved pair and `selectPair()`.

---

## Phase 6: Governance Lifecycle Completion — 3 Tasks

### Task 6.1: Finalize proposal button
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** After voting period ends (slot-based), show "Finalize" button on proposal cards.
- **Contract:** `dex_governance` has `finalize_proposal(proposal_id)` — checks quorum + threshold
- **New builder:** `buildFinalizeProposalArgs(proposalId)` — opcode + proposal_id (9 bytes)
- **Display:** Button only visible when: proposal status = active AND current slot > voting_end_slot
- **Result:** Proposal status changes to "Approved" or "Rejected"

### Task 6.2: Execute proposal button
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** After finalization with "Approved" result, show "Execute" button.
- **Contract:** `dex_governance` has `execute_proposal(proposal_id)` — cross-calls dex_core to create pair or update fees
- **New builder:** `buildExecuteProposalArgs(proposalId)` — opcode + proposal_id (9 bytes)
- **Display:** Button only visible when proposal status = approved AND not yet executed
- **Result:** Action applied (new pair listed, fees updated, etc.)

### Task 6.3: Proposal status pipeline display
- **Layer:** JS + CSS (`dex/dex.js`, `dex/dex.css`)
- **What:** Visual lifecycle pipeline on each proposal card showing stages.
- **Stages:** `Created → Voting → Finalized → Executed` (or `Rejected`)
- **CSS:** Horizontal step indicator with dots/lines, active step highlighted
- **Data:** Derive from proposal status field and timestamps

---

## Phase 7: Portfolio & Analytics — 2 Tasks

### Task 7.1: Portfolio summary in wallet panel
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** Below the balance list in wallet panel, show aggregate portfolio info.
- **Shows:**
  - Total portfolio value (sum of all balances * prices, in mUSD)
  - Total unrealized P&L across all open margin positions
  - 24h portfolio change (from cached previous values)
- **Styling:** Compact row with value + change badge

### Task 7.2: Trade history CSV export
- **Layer:** JS (`dex/dex.js`)
- **What:** "Export" icon button in Trade History tab header.
- **Generates:** CSV with columns: Date, Pair, Side, Price, Amount, Total, Fee
- **Method:** Client-side: build CSV string from displayed trades, create Blob, trigger download
- **Filename:** `moltchain-trades-{date}.csv`

---

## Phase 8: Prediction Market Completion — 1 Task

### Task 8.1: Challenge/dispute UI
- **Layer:** JS + HTML (`dex/dex.js`, `dex/index.html`)
- **What:** After market resolution is submitted, show "Challenge" button during the dispute window.
- **Contract:** `prediction_market` has `challenge_resolution(caller, market_id, proposed_outcome)` and `finalize_resolution(market_id)`
- **Display:**
  - During dispute window: "Challenge Resolution" button + outcome selector
  - After dispute window: "Finalize" button
  - Show dispute status and countdown timer
- **New builders:** `buildChallengeResolutionArgs(caller, marketId, proposedOutcome)`, `buildFinalizeResolutionArgs(marketId)`

---

## Priority Matrix

| Priority | Tasks | Why |
|----------|-------|-----|
| **P0 — Ship blockers** | 1.1-1.5, 2.1-2.4, 3.1, 3.3, 3.5, 5.1 | Position UX, stop-loss, order form safety, slippage |
| **P1 — High value** | 2.5-2.8, 3.2, 3.4, 6.1-6.2, 7.1-7.2 | TP/SL on positions, order edit, governance lifecycle, portfolio |
| **P2 — Polish** | 4.1, 4.3, 5.2-5.4, 6.3, 8.1 | Funding display, sharing, preferences, prediction disputes |
| **P3 — Future** | 4.2, 4.4 | Partial close, cross-margin design |

---

## Contract Changes Summary

| Contract | Change | New Opcodes | Byte Layout Change |
|----------|--------|-------------|-------------------|
| `dex_core` | Extend order layout +8 bytes (trigger_price), add `check_triggers` | 1 (opcode 27) | Order: +8 bytes at end |
| `dex_margin` | Add `set_position_sl_tp`, optionally `partial_close` | 1-2 (opcodes 13-14) | Position: +16 bytes (sl_price, tp_price) |
| `dex_governance` | No changes — finalize/execute already exist | 0 | None |
| `prediction_market` | No changes — challenge/finalize already exist | 0 | None |

## Tasks Requiring ZERO Contract Changes
> These only need JS/HTML/CSS + existing contract opcodes:

- **Phase 1:** Tasks 1.1-1.5 (consolidation + add/remove margin uses existing opcodes 4/5)
- **Phase 3:** Tasks 3.1-3.5 (post-only uses existing ORDER_POST_ONLY=3, cancel-all uses opcode 17, modify uses opcode 16)
- **Phase 5:** Tasks 5.1-5.4 (pure frontend/localStorage)
- **Phase 6:** Tasks 6.1-6.3 (finalize/execute already in contract)
- **Phase 7:** Tasks 7.1-7.2 (pure frontend)
- **Phase 8:** Task 8.1 (challenge/finalize already in contract)

---

## Execution Order

We execute frontend-only tasks FIRST (no contract redeploy risk), then contract changes:

### Round 1 — Frontend Only (no contract changes)
1. Phase 1: Bottom panel consolidation (Tasks 1.1-1.5)
2. Phase 3: Order form completeness (Tasks 3.1-3.5)
3. Phase 5: Settings & preferences (Tasks 5.1-5.4)
4. Phase 6: Governance lifecycle (Tasks 6.1-6.3)
5. Phase 7: Portfolio & analytics (Tasks 7.1-7.2)
6. Phase 8: Prediction challenge (Task 8.1)

### Round 2 — Contract + Frontend
7. Phase 2: Stop-loss / take-profit system (Tasks 2.1-2.8)
8. Phase 4: Margin enhancements (Tasks 4.1-4.3)

### Round 3 — Documentation
9. Task 4.4: Cross-margin design doc

---

## Task Count: 32 total
- Frontend-only: 22 tasks
- Contract + frontend: 8 tasks
- Design doc: 1 task
- Display-only (no backend): 1 task (4.1 funding rate)
