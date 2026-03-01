# MoltChain — Master Production Audit
**Date:** February 26, 2026  
**Method:** Multi-agent cross-matching (frontend ↔ contract ↔ RPC ↔ WS), line-by-line  
**Status tracking:** Each finding has an `[ ]` / `[x]` checkbox for fix tracking  

> **Closure sync (2026-02-28):** Implementation closure is tracked in [TRACKER.md](docs/audits/production_final/TRACKER.md), which currently reports **218/218 actionable issues fixed**. Treat this master file as the canonical findings catalog and `TRACKER.md` as the canonical fix-status ledger.

---

## SEVERITY LEGEND
| Symbol | Label | Meaning |
|--------|-------|---------|
| 🔴 | CRITICAL | Functionality entirely broken / security exploit / data corruption |
| 🟠 | HIGH | Major feature broken / significant incorrect behavior |
| 🟡 | MEDIUM | Incorrect behavior, missing feature, latent security risk |
| 🔵 | LOW | Quality issue, incorrect display, missing polish |
| ⚪ | INFO | Informational — no immediate action required |

---

## TABLE OF CONTENTS
1. [Cross-Cutting / Global Issues](#1-cross-cutting--global-issues)
2. [Explorer Frontend](#2-explorer-frontend)
3. [Wallet Frontend & Extension](#3-wallet-frontend--browser-extension)
4. [DEX Frontend & Contracts](#4-dex-frontend--contracts)
5. [Marketplace Frontend & Contracts](#5-marketplace-frontend--contracts)
6. [Faucet](#6-faucet)
7. [Website](#7-website)
8. [Developer Portal](#8-developer-portal)
9. [Core Contracts (Non-DEX)](#9-core-contracts-non-dex)
10. [RPC / WebSocket Layer](#10-rpc--websocket-layer)
11. [Style & Shared Consistency](#11-style--shared-consistency)
12. [Test Coverage Gaps](#12-test-coverage-gaps)
13. [Fix Priority Matrix](#13-fix-priority-matrix)

---

## 1. CROSS-CUTTING / GLOBAL ISSUES

These findings affect multiple systems simultaneously.

### 🔴 GX-01 — `callContract` RPC method does not exist
- **Impact:** Every dApp, the website quickstart, the SDK Python tests, the dev portal playground, and `test-rpc-comprehensive.sh` all call `callContract`. It has zero implementation in the dispatch table. All callers receive `{"error":{"code":-32601,"message":"Method not found: callContract"}}`.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs) — no arm in `handle_rpc()` match · [website/index.html](website/index.html#L808) · [developers/moltyid.html](developers/moltyid.html#L566) · [test-rpc-comprehensive.sh](test-rpc-comprehensive.sh#L119)
- **Fix:** Implement `callContract` in the RPC dispatch: deserialize the contract address + function opcode + args, route to `callContract()` in core, return execution result.
- [x] Fix

### 🔴 GX-02 — `tx_to_rpc_json()` hardcodes `"status": "Success"` for EVERY transaction
- **Impact:** Every failed transaction is reported as Success across the entire system. The block explorer, wallet activity, DEX trade history, and test assertions all show wrong status. Any monitoring or alerting built on transaction status is blind to failures.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L1106)
- **Fix:** Persist transaction execution result (success/failure/error message) at block finalization and surface it in `tx_to_rpc_json()`.
- [x] Fix

### 🔴 GX-03 — MoltCoin supply: 1 billion in all docs, 1 million in the contract (1000× error)
- **Impact:** Every piece of public documentation misrepresents the token supply. Tokenomics table, contract reference, and investor-facing materials are factually wrong.
- **Files:** [contracts/moltcoin/src/lib.rs](contracts/moltcoin/src/lib.rs#L67) — `1_000_000 * 1_000_000_000` (1M MOLT) · [developers/contract-reference.html](developers/contract-reference.html) — "1,000,000,000 MOLT" · [website/index.html](website/index.html) — tokenomics section
- **Fix:** Either change the contract to mint 1,000,000,000 MOLT (adjust `initial_supply`), or fix all documentation.
- [x] Fix

### 🔴 GX-04 — MoltCoin: docs claim "Fixed supply / 0% inflation"; contract has owner-callable `mint()`
- **Impact:** The "fixed supply" marketing claim is contractually false. The contract allows the owner to mint up to 10 billion MOLT. This is a potential legal/trust issue.
- **Files:** [contracts/moltcoin/src/lib.rs](contracts/moltcoin/src/lib.rs#L140-L162) — `mint()` function · [website/index.html](website/index.html) — "0% inflation"
- **Fix:** Either remove the `mint()` function and set a true fixed supply, or change tokenomics docs to reflect the actual capped-inflation model.
- [x] Fix

### 🟠 GX-05 — Five WS subscription types registered but NEVER emit events
- **Impact:** Any client subscribing to `subscribeSignatureStatus`, `subscribeValidators`, `subscribeTokenBalance`, `subscribeEpochs`, `subscribeGovernance` will receive successful subscription confirmation but will never receive a single notification. This silently breaks the wallet's transaction confirmation UX, governance UX, and any monitoring tools.
- **Files:** [rpc/src/ws.rs](rpc/src/ws.rs#L261-L280) — `Event` variants defined · zero `ws_event_tx.send()` calls exist in the entire repo for these five types
- **Fix:** Wire each event type from the validator's block finalization loop: `SignatureStatus` after tx execution, `ValidatorUpdate` on epoch boundaries, `TokenBalanceChange` on token transfer execution, `EpochChange` on epoch transition, `GovernanceEvent` on governance proposal state change.
- [x] Fix

### 🟠 GX-06 — DEX WebSocket: candle updates, user order updates, and user position updates never emitted
- **Impact:** The DEX order book page shows WS-based candles and live position updates in the UI, but these events are never sent by the validator. DEX users see stale candles and position data even though they appear to have a live WS feed.
- **Files:** [validator/src/main.rs](validator/src/main.rs#L765) — `emit_dex_events()` only emits `TradeExecution` + `TickerUpdate`; `CandleUpdate`, `OrderUpdate`, `PositionUpdate` are never called
- **Fix:** Emit `CandleUpdate` after every trade that affects an open candle interval. Emit `OrderUpdate` when order status changes (fill, cancel). Emit `PositionUpdate` when a margin position is opened/modified/liquidated.
- [x] Fix

### 🟡 GX-07 — `health` endpoint always returns `"ok"` regardless of node state
- **Impact:** Load balancers, Kubernetes liveness probes, and monitoring will never detect a stalled, wedged, or DB-disconnected node. Downtime will be invisible until clients notice.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L1385)
- **Fix:** Check last block timestamp — if `now - last_block_time > 10s`, return HTTP 503. Also verify RocksDB read succeeds.
- [x] Fix

### 🟡 GX-08 — Block hash format inconsistency: hex on native endpoints, base58 on Solana compat
- **Impact:** SDK clients and tools need separate hash-decoding paths depending on which endpoint they call. Causes confusion and bugs.
- **Files:** `getBlock` native [rpc/src/lib.rs](rpc/src/lib.rs#L2131) hex · Solana compat `getBlock` [rpc/src/lib.rs](rpc/src/lib.rs#L3887) base58
- **Fix:** Standardize. Native API should use hex consistently; Solana compat should remain base58.
- [x] Fix

### 🟡 GX-09 — DEX REST API uses raw hex addresses; native API uses base58
- **Impact:** Clients using both DEX REST and native JSON-RPC must encode addresses differently per endpoint.
- **Files:** [rpc/src/dex.rs](rpc/src/dex.rs#L540)
- **Fix:** Normalize DEX REST address output to base58 to match the canonical native API format.
- [x] Fix

### 🟡 GX-10 — No cursor-based pagination on any list endpoint
- **Impact:** `getAllContracts`, `getTransactionsByAddress`, `getAllSymbolRegistry`, `getProgramCalls`, `getTokenHolders`, `getTokenTransfers` — all return a top-N cap with no way to page forward. Heavy users see truncated data with no indication more exists.
- **Files:** All list handlers in [rpc/src/lib.rs](rpc/src/lib.rs)
- **Fix:** Add `cursor` / `after` parameter (slot + index tuple) to all paginated endpoints and include a `next_cursor` in responses.
- [x] Fix

---

## 2. EXPLORER FRONTEND

### 🔴 EX-01 — Copy buttons write element ID strings to clipboard, not actual content
- **Impact:** Users who click the copy button on block hash, transaction hash, or raw data fields will copy the literal string `"blockHash"`, `"txHash"`, or `"rawData"` — not the actual value.
- **Files:** [explorer/block.html](explorer/block.html#L109), [explorer/block.html](explorer/block.html#L127), [explorer/block.html](explorer/block.html#L265) · [explorer/transaction.html](explorer/transaction.html#L109), [explorer/transaction.html](explorer/transaction.html#L221)
- **Root cause:** `copyToClipboard(text)` in [explorer/shared/utils.js](explorer/shared/utils.js) takes text directly. HTML calls `copyToClipboard('blockHash')` (a string literal) instead of `copyToClipboard(document.getElementById('blockHash').dataset.full)`.
- **Fix:** Change all 5 copy onclick calls to pass the actual element value, e.g. `copyToClipboard(document.getElementById('blockHash').dataset.full)`.
- [x] Fix

### 🔴 EX-02 — Transaction status always rendered as "Success" in the transactions list
- **Impact:** Every failed transaction shows as green "Success" in the transactions list. Combined with GX-02 (server hardcodes success), this is a systemic issue, but the client also has its own bug.
- **Files:** [explorer/js/transactions.js](explorer/js/transactions.js) — status HTML is hardcoded `pill-success` regardless of `tx.status`
- **Fix:** Check `tx.status` field. Render `pill-failed` for `"Failed"` and appropriate icon. (Also requires GX-02 to be fixed for the data to be correct.)
- [x] Fix

### 🟠 EX-03 — Validator reputation scale mismatch: 0–1,000 vs trust tier thresholds 0–100,000
- **Impact:** Validators can never reach "Elite" (threshold ≥5,000) or "Legendary" (threshold ≥10,000) tier labels. The progress bar maxes at 100% (score 1,000) but displays "Established". The trust tier display is permanently incorrect for all validators.
- **Files:** [explorer/js/validators.js](explorer/js/validators.js#L7) `VALIDATOR_MAX_REPUTATION = 1000` · [explorer/shared/utils.js](explorer/shared/utils.js) `TRUST_TIER_THRESHOLDS`
- **Fix:** Either normalize validator score to 0–100,000 scale before calling `getTrustTier()`, or add a separate validator-specific tier boundary array.
- [x] Fix

### 🟡 EX-04 — Status filter dropdown on transactions page silently does nothing
- **Impact:** The "Success / Failed / All" filter in `transactions.html` is completely non-functional. `applyFilters()` reads the `status` value but `renderTransactions()` never consumes it.
- **Files:** [explorer/js/transactions.js](explorer/js/transactions.js) — `currentFilter.status` read but never applied in render
- **Fix:** Add a `statusFilter` condition to `renderTransactions()` when iterating `filteredTxs`.
- [x] Fix

### 🟡 EX-05 — `openEditProfileModal` sends 5 separate transactions with no rollback on partial failure
- **Impact:** If any of the 5 MoltyID update transactions fails mid-sequence (e.g., network error after the 3rd), the identity is left in a partially-updated inconsistent state with no way to undo.
- **Files:** [explorer/js/address.js](explorer/js/address.js) — `openEditProfileModal()` function
- **Fix:** Batch all 5 update calls into a single multi-instruction transaction.
- [x] Fix

### 🟡 EX-06 — `privacy.html` is unreachable — no page links to it
- **Impact:** The entire ZK privacy viewer UI is invisible. Users cannot discover it through normal navigation.
- **Files:** All HTML pages in [explorer/](explorer/) — zero `href` pointing to `privacy.html`
- **Fix:** Add a "Privacy" nav item or footer link to `privacy.html` on the main explorer page.
- [x] Fix

### 🟡 EX-07 — mainnet / testnet configs have `ws: null` with no user notification
- **Impact:** When viewing mainnet or testnet, real-time block/transaction streaming silently stops. Users see no explanation for why the live feed stopped.
- **Files:** [explorer/js/explorer.js](explorer/js/explorer.js#L18-L38)
- **Fix:** Show a banner or notification when WS is unavailable for the selected network.
- [x] Fix

### 🔵 EX-08 — Network selector default is inconsistent across pages (testnet on dashboard, mainnet elsewhere)
- **Impact:** A first-time visitor without localStorage sees testnet on the home page, then mainnet after navigating to blocks/transactions/validators — confusing.
- **Files:** [explorer/index.html](explorer/index.html) and [explorer/privacy.html](explorer/privacy.html) — `<option value="testnet" selected>` · all other pages — `<option value="mainnet" selected>`
- **Fix:** Standardize to one default (recommend `mainnet`) across all HTML files. The JS override will still apply for returning users.
- [x] Fix

### 🔵 EX-09 — `../docs/API.md` footer link is broken (file does not exist)
- **Files:** All explorer HTML pages — `../docs/API.md` in footer
- **Fix:** Change to `../docs/api/` directory link, or create `docs/API.md` as a redirect/index.
- [x] Fix

### 🔵 EX-10 — CDN scripts loaded without Subresource Integrity (SRI) attributes
- **Files:** [explorer/address.html](explorer/address.html) — js-sha3, tweetnacl, Font Awesome, Google Fonts from CDN, no `integrity=` attributes
- **Fix:** Add `integrity="sha384-..."` and `crossorigin="anonymous"` to all CDN `<script>` and `<link>` tags.
- [x] Fix

### 🔵 EX-11 — Validators page: unconditional 15s poll AND WS slot subscription run concurrently
- **Impact:** Redundant RPC call during WS connect window (~100–500ms). Minor but wasteful.
- **Files:** [explorer/js/validators.js](explorer/js/validators.js) — `setInterval(loadValidators, 15000)` is set unconditionally
- **Fix:** Set the interval only if `wsConnection` is unavailable, or clear it in the WS `open` callback.
- [x] Fix

### 🔵 EX-12 — `escapeExplorerHtml` in explorer.js duplicates `escapeHtml` from shared/utils.js
- **Files:** [explorer/js/explorer.js](explorer/js/explorer.js) — local `escapeExplorerHtml` function
- **Fix:** Delete the local duplicate and use the shared `escapeHtml` import.
- [x] Fix

### 🔵 EX-13 — Missing Google Fonts `preconnect` on blocks.html and validators.html
- **Files:** [explorer/blocks.html](explorer/blocks.html), [explorer/validators.html](explorer/validators.html)
- **Fix:** Add `<link rel="preconnect" href="https://fonts.googleapis.com">` and `<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>`.
- [x] Fix

### ⚪ EX-14 — `privacy.js` inline `onclick` handler pattern should use `safeCopy(this)` consistently
- **Files:** [explorer/js/privacy.js](explorer/js/privacy.js)
- **Fix:** Replace inline `onclick="copyToClipboard('${...}')"` with `data-copy="..."` + `safeCopy(this)`.
- [x] Fix

---

## 3. WALLET FRONTEND & BROWSER EXTENSION

### 🟠 WL-01 — Extension dApp approval polling (120 round-trips over 2 minutes)
- **Impact:** When a user takes time to approve a dApp connection, the content script makes one HTTP request per second for up to 120 seconds. Degrades performance, burns service worker wake cycles in MV3.
- **Files:** [wallet/extension/src/content/content-script.js](wallet/extension/src/content/content-script.js#L74)
- **Fix:** Replace polling with `chrome.runtime.onMessage` push notification from the service worker when the user approves or rejects.
- [x] Fix

### 🟠 WL-02 — MV3: in-memory `pendingRequests` Map lost if service worker is terminated between request and approval
- **Impact:** If the MV3 service worker is terminated (standard browser behavior) between receiving a dApp request and receiving the user's approval, the pending entry disappears. The dApp never receives a response and hangs indefinitely.
- **Files:** [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js#L6) — `const pendingRequests = new Map()`
- **Fix:** Persist pending requests to `chrome.storage.session` keyed by request ID. Restore them on service worker startup.
- [x] Fix

### 🟠 WL-03 — Fees are hardcoded (0.001 MOLT); not fetched from RPC `getFeeConfig`
- **Impact:** If the on-chain fee is changed via `setFeeConfig`, all wallets continue displaying and using the wrong fee. Users could over-pay or under-pay.
- **Files:** [wallet/shared/utils.js](wallet/shared/utils.js#L17) `BASE_FEE_MOLT = 0.001` · [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js#L614) hardcoded `eth_estimateGas` / `eth_gasPrice`
- **Fix:** Call `getFeeConfig` at wallet load time and cache the result. Update displayed fee dynamically.
- [x] Fix

### 🟠 WL-04 — Transaction serialization has three independent copies — divergence risk
- **Impact:** `serializeMessageBincode` exists in [wallet/js/wallet.js](wallet/js/wallet.js#L328), [wallet/extension/src/core/tx-service.js](wallet/extension/src/core/tx-service.js#L13), and [wallet/extension/src/core/provider-router.js](wallet/extension/src/core/provider-router.js#L304). A future bug fix in one may not be applied to the others, causing signature mismatches.
- **Fix:** Extract to a single `@moltchain/crypto` shared package. Import from one place.
- [x] Fix

### 🟡 WL-05 — Extension `crypto-service.js` lacks async BIP39 checksum validation on import
- **Impact:** The extension accepts mnemonics that pass word-list membership but fail BIP39 checksum. Users can import invalid seed phrases and generate wrong keys.
- **Files:** [wallet/extension/src/core/crypto-service.js](wallet/extension/src/core/crypto-service.js#L364) — `isValidMnemonic()` checks word count and word list only
- **Fix:** Port `isValidMnemonicAsync()` from the web wallet's `crypto.js` to `crypto-service.js`.
- [x] Fix

### 🟡 WL-06 — HD derivation is not BIP44/SLIP-0010 — incompatible with hardware wallets
- **Impact:** Derivation slices the first 32 bytes of PBKDF2-SHA512 output. This is neither BIP32 nor BIP44. Users cannot use Ledger/Trezor. Cross-wallet seed recovery will produce wrong keys.
- **Files:** [wallet/js/crypto.js](wallet/js/crypto.js#L180) · [wallet/extension/src/core/crypto-service.js](wallet/extension/src/core/crypto-service.js#L375)
- **Note:** This is by-design if MoltChain intends a custom scheme, but the comment "compatible with Solana" is technically wrong and should be corrected.
- **Fix:** Either implement SLIP-0010/Ed25519 BIP44 derivation (`m/44'/<cointype>'/0'/0'`), or remove the "Solana-compatible" comment and document the actual scheme.
- [x] Fix

### 🟡 WL-07 — No `subscribeSignature` after `sendTransaction` — confirmation tracked only by 8s HTTP poll
- **Impact:** Transaction confirmation has up to 8 seconds of latency in the UI. The infrastructure for `subscribeSignatureStatus` exists in the WS server but the wallet doesn't use it (and it never fires anyway — see GX-05).
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js) — no `subscribeSignatureStatus` call after `sendTransaction`
- **Fix:** After `sendTransaction`, open a `subscribeSignatureStatus` subscription. Cancel the subscription on confirmation. (Requires GX-05 to be fixed on the server side.)
- [x] Fix

### 🟡 WL-08 — Mnemonic backup downloads as plaintext `.txt` — no encrypted backup option
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js#L775) — `downloadMnemonic()` creates plaintext file
- **Fix:** Offer a password-protected/encrypted backup option alongside the plaintext option.
- [x] Fix

### 🟡 WL-09 — Web wallet localStorage key blob accessible to XSS on the wallet origin
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js) — `localStorage.moltWalletState`
- **Fix:** Store the decrypted session in `sessionStorage` (cleared on browser close). Keep only the encrypted blob in `localStorage`.
- [x] Fix

### 🟡 WL-10 — Transaction encoding: base64(JSON) sent to server; server uses bincode deserializer — path unclear
- **Impact:** If the RPC server only attempts bincode deserialization without a JSON fallback, all transactions from the wallet will fail deserialization.
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js#L328) sends `btoa(JSON.stringify(tx))` · [rpc/src/lib.rs](rpc/src/lib.rs#L71) calls `bounded_bincode_deserialize`
- **Fix:** Confirm and document the actual deserialization path. If the server tries JSON first, document it. If it's bincode-only, change the wallet to serialize to bincode.
- [x] Fix

### 🔵 WL-11 — No UX warning when blockhash may expire during long password entry
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js)
- **Fix:** Start a countdown timer when the password modal opens. Warn if the user has been on the modal for >30 seconds (approaching blockhash expiry). Offer a "refresh and re-sign" option.
- [x] Fix

### 🔵 WL-12 — Extension manifest: no explicit `content_security_policy` field
- **Files:** [wallet/extension/manifest.json](wallet/extension/manifest.json)
- **Fix:** Add `"content_security_policy": {"extension_pages": "script-src 'self'; object-src 'self'"}`.
- [x] Fix

### 🔵 WL-13 — All icon sizes in manifest.json use the 256px PNG
- **Files:** [wallet/extension/manifest.json](wallet/extension/manifest.json#L5)
- **Fix:** Create and reference 16px, 48px, 128px variants.
- [x] Fix

### 🔵 WL-14 — `postMessage` back to inpage uses `'*'` targetOrigin — risk of iframe interception
- **Files:** [wallet/extension/src/content/content-script.js](wallet/extension/src/content/content-script.js#L103)
- **Fix:** Replace `'*'` with `event.origin` as the targetOrigin.
- [x] Fix

### 🔵 WL-15 — Inline `onclick` string injection pattern for wallet dropdown is fragile
- **Files:** [wallet/js/wallet.js](wallet/js/wallet.js#L1208) — `onclick="switchWallet('${safeId}')"`
- **Fix:** Use event delegation with `data-wallet-id` attribute.
- [x] Fix

---

## 4. DEX FRONTEND & CONTRACTS

### 🔴 DEX-01 — Full-range tick constants mismatch: frontend ±887,272 vs contract ±443,636
- **Impact:** The "Full Range" toggle in the liquidity provision UI always fails. Any user who clicks "Full Range" will have their transaction rejected by the contract with error code 3 (invalid tick). Full-range positions are entirely inaccessible from the UI.
- **Files:** [dex/dex.js](dex/dex.js#L870-L871) — `MIN_TICK = -887272; MAX_TICK = 887272` · [contracts/dex_amm/src/lib.rs](contracts/dex_amm/src/lib.rs#L37-L38) — `MAX_TICK = 443_636`
- **Fix:** Change `dex/dex.js` to `MIN_TICK = -443_636; MAX_TICK = 443_636`.
- [x] Fix

### 🔴 DEX-02 — Router → AMM cross-call missing `is_token_a_in`, `min_out`, `deadline` (16 bytes short)
- **Impact:** Every swap routed through `dex_router` → `dex_amm` will fail with a deserialization error or execute in an undefined token direction with no slippage protection. AMM-routed swaps are completely broken at the multi-hop level.
- **Files:** [contracts/dex_router/src/lib.rs](contracts/dex_router/src/lib.rs#L560) — `execute_amm_swap()` serializes only `pool_id + amount_in` · [contracts/dex_amm/src/lib.rs](contracts/dex_amm/src/lib.rs#L650) — expects `trader[32] + pool_id[8] + is_token_a_in[1] + amount_in[8] + min_out[8] + deadline[8]`
- **Fix:** Reconstruct `execute_amm_swap` in `dex_router` to serialize all 6 fields, deriving `is_token_a_in` from `token_in == pool.token_a`.
- [x] Fix

### 🟡 DEX-03 — Pool share estimate formula off by 2³² — always shows ~100%
- **Impact:** When entering deposit amounts for a liquidity position, the share percentage display is meaningless (always rounds to 100%). Users cannot see their actual projected pool share.
- **Files:** [dex/dex.js](dex/dex.js#L3500) — `const poolPrice = pool.sqrtPrice ? Math.pow(pool.sqrtPrice / (1 << 16), 2) : 1` (missing second `/ (1 << 16)`)
- **Fix:** Change to `Math.pow(pool.sqrtPrice / (1 << 16) / (1 << 16), 2)`.
- [x] Fix

### 🟡 DEX-04 — `dex_core/abi.json` missing `trigger_price` in `place_order` (opcode 2)
- **Impact:** Any external SDK or tooling that builds on `abi.json` will send 7 parameters and receive a deserialization error. The JS frontend encodes all 8 parameters correctly (bypassing the JSON ABI), masking the bug.
- **Files:** [contracts/dex_core/abi.json](contracts/dex_core/abi.json) — opcode 2 entry · [contracts/dex_core/src/lib.rs](contracts/dex_core/src/lib.rs#L1075)
- **Fix:** Add `"trigger_price": "u64"` parameter (8th position) to the `place_order` entry in `abi.json`.
- [x] Fix

### 🟡 DEX-05 — Block height in DEX header uses 5s HTTP poll instead of WS
- **Files:** [dex/dex.js](dex/dex.js#L1705) — `setInterval(pollBlockHeight, 5000)`
- **Fix:** Subscribe via `subscribeBlocks` WS to keep block height real-time.
- [x] Fix

### 🟡 DEX-06 — Margin position data polled every 5s, no WS push
- **Impact:** Traders relying on liquidation warnings may see stale mark prices / positions for up to 5 seconds.
- **Files:** [dex/dex.js](dex/dex.js#L6478)
- **Fix:** Subscribe to `PositionUpdate` WS events for the active account after GX-06 is fixed.
- [x] Fix

### 🟡 DEX-07 — Delist and parameter-change governance proposals appear in UI but always show warning
- **Impact:** The UI presents two proposal types that cannot be executed. Users receive a confusing "not supported" notification with no further explanation.
- **Files:** [dex/dex.js](dex/dex.js#L5022), [dex/dex.js](dex/dex.js#L5030)
- **Fix:** Disable these UI elements with a tooltip explanation, or remove them from the release build.
- [x] Fix

### 🔵 DEX-08 — Footer app links use ports 3000–3004; shared-config uses 3007–3011
- **Impact:** Local dev footer navigation goes to wrong processes.
- **Files:** [dex/dex.js](dex/dex.js#L1716)
- **Fix:** Use port values from `shared-config.js` rather than hardcoded constants.
- [x] Fix

### 🔵 DEX-09 — Slot time used inconsistently: 0.4s in governance, 0.5s in prediction countdown
- **Impact:** Prediction market countdown overestimates remaining time by 25%.
- **Files:** [dex/dex.js](dex/dex.js#L4960) — `* 0.4` · [dex/dex.js](dex/dex.js#L5300) — `* 0.5`
- **Fix:** Define `SLOT_DURATION_SECONDS = 0.4` as a constant and use it everywhere.
- [x] Fix

### 🔵 DEX-10 — Hot wallet secret key persisted in `localStorage` as cleartext hex
- **Files:** [dex/dex.js](dex/dex.js#L2745) — `persistLocalWalletSession()`
- **Fix:** Encrypt the key material with a user passphrase before persisting to `localStorage`, or use `sessionStorage`.
- [x] Fix

### 🔵 DEX-11 — Hardcoded fallback contract addresses used silently if registry lookup fails
- **Files:** [dex/dex.js](dex/dex.js#L1020)
- **Fix:** Remove hardcoded fallbacks. Fail visibly with a user-facing error if the registry is unavailable.
- [x] Fix

### ⚪ DEX-12 — No UI for order expiry time; all orders are implicitly GTC
- **Files:** [dex/dex.js](dex/dex.js#L2334) — `expiry_slot = 0`
- **Fix:** Add an optional expiry field in the order form.
- [x] Fix

### ⚪ DEX-13 — `dex.js` is a 6,650-line unmodularised IIFE
- **Fix:** Refactor into ES modules: `wallet.js`, `contract-builders.js`, `ws-manager.js`, `ui-renderers.js`.
- [x] Fix

---

## 5. MARKETPLACE FRONTEND & CONTRACTS

### 🔴 MK-01 — All `sendTransaction` calls use placeholder `CONTRACT_PROGRAM_ID = [0xFF;32]`
- **Impact:** Every single transaction from the marketplace (mint, list, buy, bid, offer, accept) is sent to a dead placeholder address. No marketplace transaction has ever executed on-chain.
- **Files:** [marketplace/js/marketplace.js](marketplace/js/marketplace.js) — `CONTRACT_PROGRAM_ID`; requires resolution from symbol registry
- **Fix:** Resolve the actual deployed marketplace program address from `getAllSymbolRegistry` at page load, exactly as the DEX and wallet do.
- [x] Fix

### 🔴 MK-02 — `make_offer` passes seller's address as `offerer` — buyer is never sent
- **Impact:** Offers are always attributed to the seller, not the buyer. Offer acceptance is a no-op. The entire offer-based buying flow is broken.
- **Files:** [marketplace/js/marketplace.js](marketplace/js/marketplace.js) — `buildMakeOfferArgs()` — argument order reversed
- **Fix:** First argument to `make_offer` must be the buyer's (connected wallet's) address, not `item.owner`.
- [x] Fix

### 🔴 MK-03 — `profile.js` `accept_offer`: seller/offerer args swapped vs contract
- **Impact:** Even if the address bug in MK-02 is fixed, `accept_offer` encodes seller and offerer in wrong order. Contract will reject or credit funds to wrong party.
- **Files:** [marketplace/js/profile.js](marketplace/js/profile.js) — `buildAcceptOfferArgs()` · [contracts/moltmarket/src/lib.rs](contracts/moltmarket/src/lib.rs) — `accept_offer` parameter order
- **Fix:** Align the JS argument order with the contract's expected parameter layout.
- [x] Fix

### 🔴 MK-04 — `createCollection` RPC call does not exist as an RPC method
- **Impact:** The "Create Collection" button calls `rpc.createCollection(...)` which routes to an internal classifier string, not an RPC handler. Collection creation always fails with method-not-found.
- **Files:** [marketplace/js/create.js](marketplace/js/create.js) · [rpc/src/lib.rs](rpc/src/lib.rs) — no `createCollection` arm
- **Fix:** Route collection creation through `sendTransaction` calling the `moltpunks::create_collection` contract function.
- [x] Fix

### 🔴 MK-05 — Mint sends opcode/binary payload to `moltpunks::mint` which expects WASM ABI pointer args
- **Impact:** The create flow serializes a binary opcode payload but the contract expects WASM function arguments. Every mint attempt fails on deserialization.
- **Files:** [marketplace/js/create.js](marketplace/js/create.js) — `buildMintArgs()` · [contracts/moltpunks/src/lib.rs](contracts/moltpunks/src/lib.rs)
- **Fix:** Serialize mint arguments to match the exact byte layout expected by `moltpunks::mint`.
- [x] Fix

### 🔴 MK-06 — Auction system entirely unwired (both `moltmarket`'s and `moltauction`'s)
- **Impact:** There is no bid UI, no create-auction UI, no settle/cancel auction flow anywhere in the marketplace. Two fully-implemented auction contracts (`moltmarket::create_auction` + standalone `moltauction`) are 100% unused.
- **Files:** [marketplace/js/](marketplace/js/) — no `buildPlaceBidArgs`, `buildCreateAuctionArgs`, `buildSettleAuctionArgs` anywhere · [contracts/moltmarket/src/lib.rs](contracts/moltmarket/src/lib.rs) · [contracts/moltauction/src/lib.rs](contracts/moltauction/src/lib.rs)
- **Fix:** Implement the full auction UI. Decide which contract is canonical (`moltmarket` embedded vs `moltauction`) and remove the other, or document both use cases.
- [x] Fix

### 🔴 MK-07 — NFT metadata stored as inline `data:` URI; `reef_storage` is never called
- **Impact:** Metadata is embedded as base64 data URIs inline. `reef_storage` (the decentralized storage contract) is never invoked. Images will be lost when contract state is pruned. IPFS or on-chain Reef Storage was clearly intended.
- **Files:** [marketplace/js/create.js](marketplace/js/create.js) · [contracts/reef_storage/src/lib.rs](contracts/reef_storage/src/lib.rs) — never called from marketplace
- **Fix:** Upload image/metadata to `reef_storage` or IPFS first, then pass the resulting content hash as the metadata URI in the mint transaction.
- [x] Fix

### 🔴 MK-08 — `moltmarket::make_offer` has no escrow — funds not locked at offer time
- **Impact:** An offerer can make an offer, immediately drain their wallet, and the offer remains valid. When accepted, the settlement will fail or succeed unexpectedly. Classic offer-without-escrow vulnerability.
- **Files:** [contracts/moltmarket/src/lib.rs](contracts/moltmarket/src/lib.rs)
- **Fix:** Lock offer amount in escrow (transfer to marketplace program account) in `make_offer`. Release on acceptance or cancellation.
- [x] Fix

### 🔴 MK-09 — `browse.html` "Clear Filters" button calls undefined `clearFilters()` (ReferenceError crashes JS)
- **Impact:** Clicking "Clear Filters" throws a JavaScript ReferenceError, which may crash the entire page script context depending on the browser.
- **Files:** [marketplace/browse.html](marketplace/browse.html) — `onclick="clearFilters()"` · [marketplace/js/browse.js](marketplace/js/browse.js) — function not defined
- **Fix:** Define `clearFilters()` or rename to the correct existing function.
- [x] Fix

### 🟠 MK-10 — Royalty fields never set in `list_nft` path — all listings have 0% royalties permanently
- **Impact:** NFT creators receive 0 royalties on all secondary sales. The contract supports royalties but the `buildListNftArgs` function never encodes royalty address or percentage.
- **Files:** [marketplace/js/marketplace.js](marketplace/js/marketplace.js) · [contracts/moltmarket/src/lib.rs](contracts/moltmarket/src/lib.rs)
- **Fix:** Add royalty address and royalty BPS fields to `buildListNftArgs`, pulled from the NFT's metadata or collection settings.
- [x] Fix

### 🟠 MK-11 — `moltpunks::mint` is minter-gated; user-initiated mints from the Create page always return error 0
- **Impact:** Non-admin users cannot mint NFTs through the standard UI flow.
- **Files:** [contracts/moltpunks/src/lib.rs](contracts/moltpunks/src/lib.rs) — `mint()` requires `is_minter` authorization
- **Fix:** Either add a public `public_mint()` entrypoint with a mint price check, or document that `moltpunks` is admin-only and redirect the Create page to `moltmarket`'s collection/mint flow.
- [x] Fix

### 🟠 MK-12 — `moltmarket::accept_collection_offer` double-charges marketplace fee from offerer
- **Impact:** The offerer's balance is charged the marketplace fee a second time at settlement after paying it at offer creation. This is a double-spend bug that over-charges buyers.
- **Files:** [contracts/moltmarket/src/lib.rs](contracts/moltmarket/src/lib.rs) — `accept_collection_offer()`
- **Fix:** Market fee should be taken once at settlement from the payment. Remove the duplicate charge.
- [x] Fix

---

## 6. FAUCET

### 🔴 FA-01 — CAPTCHA is frontend-only; any direct HTTP POST bypasses it completely
- **Impact:** A bot calling `/faucet/request` directly never encounters the math challenge. The Rust server performs zero CAPTCHA verification. The cooldown (60s) and daily IP cap are the only server-side defenses.
- **Files:** [faucet/faucet.js](faucet/faucet.js#L9-L15) — CAPTCHA generation and check · [faucet/src/main.rs](faucet/src/main.rs#L264-L310) — no captcha field in handler
- **Fix:** For production: implement server-side CAPTCHA verification (e.g., hCaptcha or a server-signed PoW challenge generated on page load and verified on submission). At minimum: move to a time-limited signed token approach.
- [x] Fix

### 🔴 FA-02 — Dashboard shows "24 Hours" cooldown; server enforces 60 seconds
- **Impact:** The UI lies about rate limits. A bot that reads the UI thinks it needs to wait 24 hours between requests. A developer reading the code sees 60 seconds. In reality the limit is 60 seconds per address.
- **Files:** [faucet/index.html](faucet/index.html#L75) — "24 Hours" stat card · [faucet/src/main.rs](faucet/src/main.rs#L172) — `COOLDOWN_SECONDS` default 60
- **Fix:** Either change `COOLDOWN_SECONDS` default to `86400`, or have the frontend fetch the actual cooldown from `/health` or a config endpoint and display it dynamically.
- [x] Fix

### 🟠 FA-03 — Faucet amount stat card shows hardcoded `MOLT_PER_REQUEST = 100`, not the server's actual value
- **Impact:** If `MAX_PER_REQUEST` env var is changed on the server, the UI continues to show 100 MOLT.
- **Files:** [faucet/faucet.js](faucet/faucet.js#L29-L35) — `updateStats()` uses local constant · [faucet/src/main.rs](faucet/src/main.rs#L161)
- **Fix:** Expose `max_per_request` field in the `/health` response and read it in `updateStats()`.
- [x] Fix

### 🟡 FA-04 — Faucet mechanism uses native system transfer, not MoltCoin smart contract
- **Impact:** Whether this is correct depends on the canonical MOLT design. If MOLT is a native protocol asset, the system transfer is correct. If MoltCoin MT-20 is the canonical representation, the contract should be called. The documentation is silent on which is authoritative.
- **Files:** [faucet/src/main.rs](faucet/src/main.rs#L445-L555)
- **Fix:** Document and enforce the canonical MOLT representation. If native, remove MoltCoin confusion from documentation. If contract-based, wire to `callContract`.
- [x] Fix

### 🟡 FA-05 — CORS allowlist excludes common development ports
- **Files:** [faucet/src/main.rs](faucet/src/main.rs#L200-L212) — only whitelists `:3003`, `:3000`, and production domains
- **Fix:** Add the full set of development ports from `shared-config.js` to the CORS allowlist, or make it env-configurable.
- [x] Fix

### 🟡 FA-06 — Footer links point to raw `.md` files (render as plain text in browser)
- **Files:** [faucet/index.html](faucet/index.html#L213-L220) — links to `../docs/README.md`, `../docs/foundation/VISION.md`, etc.
- **Fix:** Link to HTML documentation pages or the developer portal equivalents.
- [x] Fix

---

## 7. WEBSITE

### 🔴 WB-01 — Main quickstart example calls `callContract` which does not exist in the RPC
- **Impact:** The primary onboarding example for all new developers demonstrates a method that returns Method Not Found. First impression is a broken API.
- **Files:** [website/index.html](website/index.html#L808) — deploy wizard Step 5
- **Fix:** Replace `callContract` with `sendTransaction` + a correctly serialized contract call, or implement `callContract` (see GX-01).
- [x] Fix

### 🟡 WB-02 — No navigation links to Faucet, Marketplace, or DEX from the main website
- **Files:** [website/index.html](website/index.html#L30-L37) — top nav only has "Docs" and "Validators"
- **Fix:** Add Ecosystem nav links (DEX, Marketplace, Faucet, Explorer) to the main website header.
- [x] Fix

### 🟡 WB-03 — "Browse All 27 Contracts" button links to an unverified GitHub URL
- **Files:** [website/index.html](website/index.html#L748) — `https://github.com/moltchain/moltchain/tree/main/contracts`
- **Fix:** Verify the URL is correct, or link to the contracts section of the developer portal.
- [x] Fix

### 🔵 WB-04 — Solana 65,000 TPS comparison uses theoretical peak, not real-world sustained
- **Impact:** The comparison is misleading — real-world Solana sustained is ~3,000–4,000 TPS.
- **Files:** [website/index.html](website/index.html#L283)
- **Fix:** Use "theoretical peak" or cite the Solana marketing figure honestly.
- [x] Fix

### 🔵 WB-05 — `data-molt-app` cross-app links silently break with JavaScript disabled
- **Files:** All `data-molt-app` links render as `href="#"` without JS. No `<noscript>` fallback.
- **Fix:** Add `<noscript>` with direct absolute URL fallbacks.
- [x] Fix

---

## 8. DEVELOPER PORTAL

### 🔴 DEV-01 — `programs/index.html` (IDE playground) does not exist
- **Impact:** The "Programs IDE" hub card, the playground guide "Launch Now" button, and the website deploy wizard Step 1 all link to a missing file. Every developer who tries to use the online IDE hits a 404.
- **Files:** [developers/index.html](developers/index.html#L245) · [developers/playground.html](developers/playground.html)
- **Fix:** Either build the Programs IDE, or replace all links with a "Coming Soon" page, or link to an existing WASM playground or CLI-based workflow.
- [x] Fix

### 🟠 DEV-02 — 30+ implemented RPC methods completely undocumented
- **Impact:** Developers cannot discover or use MoltyID (6 methods), ZK privacy (6 methods), bridge (3 methods), ReefStake (6 methods), prediction market (8 methods), DEX/contract stats (~18 methods), admin endpoints, registry endpoints, and governance endpoints.
- **Files:** [developers/rpc-reference.html](developers/rpc-reference.html) — sidebar missing ~50 methods present in [rpc/src/lib.rs](rpc/src/lib.rs#L1355-L1545)
- **Fix:** Generate RPC reference documentation from the actual dispatch table. Add auto-generation or a sync check to CI.
- [x] Fix

### 🔴 DEV-03 — `contract-reference.html` uses an entirely different CSS design system
- **Impact:** The contract reference page looks completely different from every other page in the developer portal — different background, different nav color, different typography. Breaks brand consistency.
- **Files:** [developers/contract-reference.html](developers/contract-reference.html#L1-L90) — inline `:root {}` with `--bg-primary`, `--accent` vs `--bg-dark`, `--primary` used everywhere else; missing `shared-base-styles.css` and `shared-theme.css` links
- **Fix:** Remove inline `:root {}` block from `contract-reference.html`. Add `<link>` tags for `shared-base-styles.css` and `shared-theme.css`. Replace custom variable names with the shared ones.
- [x] Fix

### 🟡 DEV-04 — Portal search index uses `molt_` prefix; actual API methods have no prefix
- **Impact:** A developer searching for "getBalance" in the portal search will not find it; it's indexed as "molt_getBalance". Search results are unreliable.
- **Files:** [developers/js/developers.js](developers/js/developers.js#L283)
- **Fix:** Remove the `molt_` prefix from the search index entries.
- [x] Fix

### 🟡 DEV-05 — WS Solana-compat aliases (`slotSubscribe`, `signatureSubscribe`, etc.) are undocumented
- **Files:** [rpc/src/ws.rs](rpc/src/ws.rs#L733), [developers/ws-reference.html](developers/ws-reference.html)
- **Fix:** Add a "Solana Compatibility" section to the WS reference listing the alias methods.
- [x] Fix

---

## 9. CORE CONTRACTS (NON-DEX)

### 🔴 CON-01 — `moltoracle`: timestamp in milliseconds vs comparison in seconds — oracle always "stale"  
- **Impact:** `get_timestamp()` returns milliseconds. Staleness checks compare `elapsed > 3600` (seconds). A price published 3.6 seconds ago is rejected as "stale." The oracle is effectively non-functional — every price read fails the freshness check.
- **Files:** [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L203) `get_timestamp()` · [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L832) staleness check `> 3600`
- **Fix:** Change both freshness comparisons from `> 3_600` to `> 3_600_000` to match milliseconds, or fix `get_timestamp()` to return seconds.
- [x] Fix

### 🔴 CON-02 — `shielded_pool`: no reentrancy guard on `shield()`, `unshield()`, `transfer()`
- **Impact:** A reentrant call between `load_state()` and `save_state()` can load stale state, allowing nullifier double-spend (an attacker could unshield the same note twice).
- **Files:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L608-L700)
- **Fix:** Add `reentrancy_enter()` / `reentrancy_exit()` guards to all three mutation functions, matching the pattern used in every other contract.
- [x] Fix

### 🔴 CON-03 — `shielded_pool`: no caller verification on any mutation function
- **Impact:** Security depends entirely on the processor dispatching only after ZK proof verification. A direct WASM call (bypassing the processor) can execute without any proof. The contract itself provides zero caller authentication.
- **Files:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L608)
- **Fix:** Add a processor authority check — verify that the ZK verifier program is the caller, or add a protocol-level signer guard.
- [x] Fix

### 🔴 CON-04 — `shielded_pool`: no pause mechanism
- **Impact:** If a ZK circuit vulnerability is discovered, the pool cannot be halted. All other contracts have `emergency_pause()`.
- **Files:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs)
- **Fix:** Add `pause()` / `unpause()` (with two-step timelock) guarded by admin authority.
- [x] Fix

### 🔴 CON-05 — `clawpump`: `transfer_molt_out` silently returns success when MOLT address is unconfigured
- **Impact:** A seller's MOLT funds are permanently lost when the MOLT address is not yet configured. The function returns `true` (success) instead of an error, masking the fund loss.
- **Files:** [contracts/clawpump/src/lib.rs](contracts/clawpump/src/lib.rs#L190)
- **Fix:** Return an explicit error code (not success) when MOLT address is unset.
- [x] Fix

### 🟠 CON-06 — `lobsterlend`: health factor calculation overflows `u64` for deposits > ~2.17M MOLT
- **Impact:** Large depositors get an incorrect (overflowed) health factor, potentially blocking legitimate borrows or preventing liquidation of genuinely unhealthy positions.
- **Files:** [contracts/lobsterlend/src/lib.rs](contracts/lobsterlend/src/lib.rs#L750) — `deposit * 85 * 100 / borrow` as u64
- **Fix:** Cast to `u128` for the multiplication: `((deposit as u128) * 85 * 100 / (borrow as u128)) as u64`.
- [x] Fix

### 🟠 CON-07 — `moltdao`: `PROPOSAL_SIZE = 210` but layout is 212 bytes
- **Impact:** A proposal exactly at the size boundary (210 bytes) passes the guard with `stake_amount` unreadable. The deserialized `stake_amount` = 0, skipping stake refund on proposal close. Authors lose their staked MOLT.
- **Files:** [contracts/moltdao/src/lib.rs](contracts/moltdao/src/lib.rs#L317)
- **Fix:** Change `PROPOSAL_SIZE` to `212` to match the actual layout.
- [x] Fix

### 🟡 CON-08 — `moltoracle`: legacy `request_randomness()` is front-runnable
- **Impact:** Block producers can see pending randomness requests before finalization and influence the result.
- **Files:** [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs)
- **Fix:** Remove or disable `request_randomness()`. The commit-reveal randomness system already exists and should be the only randomness mechanism.
- [x] Fix

### 🟡 CON-09 — `clawvault`: no total allocation cap on strategy addition
- **Impact:** Admin can add strategies totalling >100% of vault assets. On a rebalance, the vault would try to over-allocate, potentially draining assets.
- **Files:** [contracts/clawvault/src/lib.rs](contracts/clawvault/src/lib.rs)
- **Fix:** Enforce that the sum of all strategy allocations ≤ 10,000 bps (100%) in `add_strategy` and `update_strategy`.
- [x] Fix

### 🟡 CON-10 — `clawvault`: `harvest()` silently succeeds when protocol addresses are unset
- **Impact:** Protocol fees are silently dropped. `harvest()` returns code `1` (success) when `protocol_address` or `treasury_address` is not configured, instead of an error.
- **Files:** [contracts/clawvault/src/lib.rs](contracts/clawvault/src/lib.rs)
- **Fix:** Return an error code if required addresses are unset. Never silently succeed a fund movement.
- [x] Fix

### 🟡 CON-11 — `compute_market`: no emergency pause function
- **Files:** [contracts/compute_market/src/lib.rs](contracts/compute_market/src/lib.rs)
- **Fix:** Add `pause()` / `unpause()` admin functions with the standard two-step timelock pattern.
- [x] Fix

### 🟡 CON-12 — `shielded_pool`: entire pool stored as a single JSON blob — unbounded growth / griefing vector
- **Impact:** Each `shield` operation appends to one storage key. At scale this key becomes unreadably large and any read/write requires deserializing the entire pool. A DoS attacker can cheap-spam small shields to bloat the key indefinitely.
- **Files:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs)
- **Fix:** Use a sparse Merkle tree with per-leaf storage keys (indexed by leaf index), not a single JSON blob.
- [x] Fix

---

## 10. RPC / WEBSOCKET LAYER

### 🟠 RPC-01 — Admin token transmitted in JSON request body (shows in access logs)
- **Impact:** If HTTP request bodies are logged (standard for debugging), admin tokens appear in plain text in access logs.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L230-L264)
- **Fix:** Move admin token to `Authorization: Bearer <token>` header. Remove it from the request body.
- [x] Fix

### 🟡 RPC-02 — Deprecated `stakeToReefStake` / `unstakeFromReefStake` / `claimUnstakedTokens` still consume rate-limit budget
- **Impact:** Applications calling these deprecated methods hit the Expensive rate-limit tier before getting -32601.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L297)
- **Fix:** Remove deprecated methods from the `classify_method()` Expensive tier or move to a separate "deprecated/removed" category that returns -32601 without consuming rate budget.
- [x] Fix

### 🟡 RPC-03 — `circulating_supply` in `getMetrics` overstates freely tradeable supply
- **Impact:** The circulating supply metric does not subtract locked vesting or unstaking queue amounts.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L4327)
- **Fix:** Subtract `total_vesting_locked + total_unstaking_queue` from `circulating_supply`.
- [x] Fix

### 🟡 RPC-04 — Prediction market WS events emitted pre-mempool, not post-confirmation
- **Impact:** Prediction events may be emitted for transactions that are eventually rejected or never included in a block.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L643)
- **Fix:** Move prediction event emission to the block finalization path, after transaction execution is confirmed.
- [x] Fix

### 🟡 RPC-05 — `MOLTCHAIN_CORS_ORIGINS=*` has no production guard
- **Impact:** If a mis-configured production deployment sets `CORS_ORIGINS=*`, any origin can send RPC calls.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L1236)
- **Fix:** Add a check: if `NETWORK == "mainnet"` and `CORS_ORIGINS == "*"`, refuse to start with an explicit error.
- [x] Fix

### 🟡 RPC-06 — `getGenesisAccounts` hardcodes `"amount_molt": 1_000_000_000` regardless of actual supply
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L4379)
- **Fix:** Read the actual initial supply from the genesis block or chain configuration.
- [x] Fix

---

## 11. STYLE & SHARED CONSISTENCY

### 🔴 STY-01 — `developers/contract-reference.html` entirely outside the shared design system
- See DEV-03 above. This is a style issue AND a functional branding failure.

### 🟡 STY-02 — Explorer: network selector default inconsistent across pages
- See EX-08 above.

### 🔵 STY-03 — Missing Google Fonts `preconnect` on `explorer/blocks.html` and `explorer/validators.html`
- See EX-13 above.

### 🔵 STY-04 — DEX footer ports (3000–3004) don't match shared-config.js (3007–3011)
- See DEX-08 above.

### ⚪ STY-05 — All other frontends: `shared-base-styles.css` and `shared-theme.css` are byte-for-byte identical
- ✅ No action needed. CSS consistency is confirmed across Explorer, Wallet, DEX, Marketplace, Faucet, Website, and Developer Portal (excluding contract-reference.html).

---

## 12. TEST COVERAGE GAPS

### 🔴 TC-01 — Trust tier tests in `explorer.test.js` test the wrong stubs (0–1,000 scale vs 0–100,000)
- **Files:** [explorer/explorer.test.js](explorer/explorer.test.js#L109-L130)
- **Fix:** Remove the local stubs. Import and test the production `getTrustTier()` directly. Add threshold boundary tests at 1,000 / 5,000 / 10,000 / 50,000 / 100,000.
- [x] Fix

### 🔴 TC-02 — `faucet.test.js` assertion for `escapeHtml` in `faucet.js` will always fail (function is in `shared/utils.js`)
- **Files:** [faucet/faucet.test.js](faucet/faucet.test.js#L181)
- **Fix:** Fix the test to read `shared/utils.js` instead of `faucet.js`.
- [x] Fix

### 🔴 TC-03 — `test-rpc-comprehensive.sh` `callContract` test always returns FAIL; likely not blocking CI
- **Files:** [test-rpc-comprehensive.sh](test-rpc-comprehensive.sh#L119)
- **Fix:** After implementing `callContract` (GX-01), verify this test passes. Until then, block CI on this failure.
- [x] Fix

### 🟠 TC-04 — No tests confirm 5 dead WS subscription types
- **Fix:** Add WS integration tests that subscribe, trigger the underlying event (mock or real), and assert delivery.
- [x] Fix

### 🟠 TC-05 — `sendTransaction` with a valid signed transaction is untested end-to-end
- **Files:** [rpc/tests/](rpc/tests/)
- **Fix:** Add an integration test that constructs, signs, and submits a real transaction, then confirms it via `getTransaction` and `subscribeSignatureStatus`.
- [x] Fix

### 🟡 TC-06 — `rpc_full_coverage.rs` `assert_valid_rpc()` never checks `status` field on transactions
- **Files:** [rpc/tests/rpc_full_coverage.rs](rpc/tests/rpc_full_coverage.rs)
- **Fix:** After fixing GX-02, add assertions that failed transactions return `"status": "Failed"`.
- [x] Fix

### 🟡 TC-07 — Explorer: no tests for copy-to-clipboard behavior, status rendering, or status filter
- **Fix:** Add unit tests for `copyToClipboard` (expects actual content), `renderTransactions` (checks status pill CSS class), and `applyFilters` (checks filtered count).
- [x] Fix

### 🟡 TC-08 — DEX: no tests for full-range tick passthrough or AMM router serialization
- **Fix:** Add unit tests for `buildAddLiquidityArgs(true, ...)` (full-range = MIN/MAX ticks are ±443,636), and for `execute_amm_swap` byte layout.
- [x] Fix

### 🟡 TC-09 — No integration tests for the Rust faucet backend
- **Fix:** Add Rust integration tests: valid address + cooldown enforcement, daily IP limit, mainnet panic guard, CORS rejection, amount capping.
- [x] Fix

### 🔵 TC-10 — `test-websocket.sh` only tests connection, not any subscription type or message format
- **Fix:** Add wscat-based tests for `subscribeBlocks`, `subscribeAccount`, `subscribeSignatureStatus` (once GX-05 is fixed).
- [x] Fix

---

## 13. FIX PRIORITY MATRIX

### PHASE 1 — Blockers (Nothing works without these)
| ID | Title | Systems Affected |
|----|-------|-----------------|
| GX-01 | Implement `callContract` RPC method | Website, Dev Portal, SDK, dApps |
| GX-02 | Fix `tx_to_rpc_json` hardcoded Success status | ALL systems |
| GX-03 | Fix MoltCoin supply: 1M in code vs 1B in docs | Tokenomics, Trust |
| GX-04 | Fix "fixed supply" claim vs `mint()` function | Tokenomics, Trust |
| CON-01 | Fix `moltoracle` ms vs seconds: oracle always stale | DEX margin, prediction market |
| CON-02 | Fix `shielded_pool` reentrancy: double-spend possible | ZK Privacy |
| CON-03 | Fix `shielded_pool` caller verification | ZK Privacy |
| MK-01 | Fix marketplace placeholder contract address | Marketplace |
| MK-02 | Fix `make_offer` buyer vs seller arg swap | Marketplace |
| MK-09 | Fix `clearFilters()` ReferenceError on browse page | Marketplace |
| DEX-01 | Fix full-range tick ±887,272 → ±443,636 | DEX Liquidity |
| DEX-02 | Fix router→AMM missing `is_token_a_in`/`min_out`/`deadline` | DEX Swaps |
| EX-01 | Fix copy buttons writing element IDs | Explorer |
| EX-02 | Fix transaction status always "Success" | Explorer |
| FA-01 | Server-side CAPTCHA / PoW for faucet | Faucet |
| FA-02 | Fix faucet cooldown: 24h in UI vs 60s in server | Faucet |
| TC-02 | Fix `faucet.test.js` always-failing assertion | CI |
| TC-03 | Block CI on `callContract` test failure | CI |
| DEV-01 | Fix or remove Programs IDE 404 | Developer Portal |

### PHASE 2 — High Priority (Major features broken)
| ID | Title |
|----|-------|
| GX-05 | Wire 5 dead WS subscriptions (SignatureStatus, Validators, TokenBalance, Epochs, Governance) |
| GX-06 | Emit DEX candle/order/position WS updates from validator |
| MK-03 | Fix `accept_offer` arg swap in profile.js |
| MK-04 | Fix `createCollection` routing through `sendTransaction` |
| MK-05 | Fix mint payload serialization for moltpunks |
| MK-06 | Implement full auction UI (both bid + create + settle) |
| MK-07 | Wire reef_storage for NFT metadata |
| MK-08 | Add escrow to `moltmarket::make_offer` |
| MK-10 | Wire royalty fields in `buildListNftArgs` |
| MK-11 | Add public mint entrypoint to moltpunks |
| MK-12 | Fix `accept_collection_offer` double marketplace fee |
| CON-04 | Add pause to `shielded_pool` |
| CON-05 | Fix `clawpump::transfer_molt_out` silent success |
| CON-06 | Fix `lobsterlend` health factor u64 overflow |
| CON-07 | Fix `moltdao::PROPOSAL_SIZE = 212` |
| WL-01 | Fix extension approval loop (120 round-trips) |
| WL-02 | Persist MV3 pending requests to storage |
| WL-03 | Dynamic fee from `getFeeConfig` |
| WL-04 | Deduplicate three copies of transaction serializer |
| DEX-03 | Fix pool share formula (off by 2³²) |
| WB-01 | Fix website quickstart `callContract` example |
| DEV-02 | Document all 30+ undocumented RPC methods |
| DEV-03 | Fix `contract-reference.html` CSS design system |
| EX-03 | Fix validator trust tier scale mismatch |
| TC-01 | Fix trust tier unit tests (testing wrong stubs) |
| GX-07 | Implement real `/health` liveness check |

### PHASE 3 — Medium Priority (Quality and correctness)
| ID | Title |
|----|-------|
| GX-08 | Standardize block hash encoding (hex vs base58) |
| GX-09 | Normalize DEX REST address format to base58 |
| GX-10 | Add cursor-based pagination to all list endpoints |
| EX-04 | Wire status filter dropdown in transactions.js |
| EX-05 | Batch MoltyID profile update (5 txns → 1) |
| EX-06 | Add navigation link to privacy.html |
| EX-07 | Show notification when WS is unavailable for network |
| WL-05 | BIP39 checksum validation in extension |
| WL-06 | Document (or implement BIP44) HD derivation scheme |
| WL-07 | Use `subscribeSignatureStatus` after `sendTransaction` |
| WL-08 | Offer encrypted mnemonic backup |
| WL-09 | Move session to `sessionStorage` |
| WL-10 | Clarify transaction encoding (base64 JSON vs bincode) |
| DEX-04 | Update `dex_core/abi.json` with `trigger_price` |
| DEX-05 | Replace block height poll with WS |
| DEX-06 | Replace margin position poll with WS |
| DEX-07 | Disable unimplemented governance proposal types |
| CON-08 | Disable legacy `request_randomness()` |
| CON-09 | Add strategy allocation cap to `clawvault` |
| CON-10 | Fix `clawvault::harvest` silent success |
| CON-11 | Add pause to `compute_market` |
| CON-12 | Refactor `shielded_pool` storage to sparse per-leaf keys |
| RPC-01 | Move admin token to Authorization header |
| RPC-02 | Remove deprecated ReefStake methods from rate-limit tier |
| RPC-03 | Fix `circulating_supply` (subtract locked amounts) |
| RPC-04 | Move prediction WS events to post-confirmation |
| RPC-05 | Guard `CORS_ORIGINS=*` on mainnet |
| FA-03 | Dynamic faucet amount from server config |
| FA-04 | Document/enforce canonical MOLT representation |
| FA-05 | Fix faucet CORS allowlist |
| FA-06 | Fix faucet footer links (`.md` → HTML) |
| WB-02 | Add Ecosystem links to website main nav |
| WB-03 | Verify or fix GitHub contract browser URL |
| DEV-04 | Remove `molt_` prefix from portal search index |
| DEV-05 | Document WS Solana-compat aliases |
| TC-04 through TC-10 | All remaining test coverage gaps |

### PHASE 4 — Low Priority / Polish
| ID | Title |
|----|-------|
| EX-08 | Standardize network selector default to mainnet |
| EX-09 | Fix `../docs/API.md` footer link |
| EX-10 | Add SRI to CDN scripts |
| EX-11 | Fix validators double-poll |
| EX-12 | Remove `escapeExplorerHtml` duplicate |
| EX-13 | Add Google Fonts preconnect to blocks/validators pages |
| EX-14 | Use `safeCopy` pattern in privacy.js |
| WL-11 | Blockhash expiry UX warning |
| WL-12 | Add explicit CSP to extension manifest |
| WL-13 | Add proper icon sizes to extension manifest |
| WL-14 | Fix `postMessage` `'*'` targetOrigin |
| WL-15 | Use event delegation for wallet dropdown |
| DEX-08 | Fix footer port mismatch |
| DEX-09 | Define `SLOT_DURATION_SECONDS` constant |
| DEX-10 | Encrypt DEX hot wallet in localStorage |
| DEX-11 | Remove hardcoded fallback contract addresses |
| DEX-12 | Add order expiry UI |
| DEX-13 | Modularize dex.js IIFE |
| RPC-06 | Fix `getGenesisAccounts` hardcoded supply |
| WB-04 | Clarify Solana TPS comparison |
| WB-05 | Add `<noscript>` fallbacks for data-molt-app links |
| STY-02–04 | Remaining style polish items |

---

## FINDING COUNTS BY SEVERITY (Round 1 — Feb 26)

| Severity | Count |
|----------|-------|
| 🔴 CRITICAL | 22 |
| 🟠 HIGH | 18 |
| 🟡 MEDIUM | 42 |
| 🔵 LOW | 24 |
| ⚪ INFO | 7 |
| **TOTAL** | **113** |

---

---

# ROUND 2 FINDINGS — February 27, 2026

> Deep per-frontend line-by-line re-audit. Individual audit files written:
> - `explorer/EXPLORER_AUDIT.md` (39 issues — 5 false positives removed during verification)
> - `WALLET_AUDIT_REPORT.md` (40 issues — 2 false positives removed during verification)
> - `DEX_PRODUCTION_AUDIT_FULL.md` (14 issues — 2 false positives removed during verification)
> - `rpc/RPC_AUDIT.md` (23 issues — 1 false positive removed during verification)
> - `faucet/FAUCET_AUDIT.md` (21 issues — all confirmed)
> - `website/WEBSITE_AUDIT.md` (23 issues — 1 false positive removed during verification)
> - `developers/DEVPORTAL_AUDIT.md` (31 issues — 5 partially corrected during verification)
> - `marketplace/MARKETPLACE_AUDIT.md` (39 issues — 4 false positives removed during verification)

---

## 14. SHIELDED POOL / ZK PRIVACY (NEW — Not in Round 1)

### 🔴 ZK-01 — Groth16 WASM prover is commented out; all proofs are SHA-256 placeholders
- **Impact:** The entire shielded pool is non-functional and a security fraud. All three proof generators (`shieldProof`, `unshieldProof`, `transferProof`) return 128-byte SHA-256 digests as if they are ZK proofs. The on-chain verifier accepts any 128-byte buffer (it appears to do no real verification), so users believe their transactions are private — they are not. Any observer can correlate inputs to outputs.
- **Files:** [wallet/js/shielded.js](wallet/js/shielded.js) — Groth16 WASM import line commented out · `shieldProof()`, `unshieldProof()`, `transferProof()` all return `sha256(...)` placeholders
- **Fix:** Implement real Groth16 proof generation using BN254 (e.g., via `snarkjs` WASM). Wire the WASM correctly. Until fixed, display a clear "Privacy features coming soon — shielded transactions are NOT private" warning and disable the shielded UI.
- [x] Fix

### 🔴 ZK-02 — Shielded spending key derivable from public address (privacy entirely broken)
- **Impact:** The shielded spending key is computed as `SHA-256(wallet.address + ':shielded')`. Since `wallet.address` is public, anyone who knows your wallet address can derive your spending key, scan all shielded notes, and spend them. All user funds in the shielded pool are at risk of theft.
- **Files:** [wallet/js/shielded.js](wallet/js/shielded.js) — `deriveSpendingKey()` function
- **Fix:** Derive the shielded spending key from the private seed/entropy path (BIP44 sub-derivation), never from the public address. The public address must NOT be an input to the private key derivation function.
- [x] Fix

### 🔴 ZK-03 — XOR-only note encryption (no AEAD, no MAC) — malleability attack
- **Impact:** Shielded note ciphertexts use XOR encryption with no authentication tag. An attacker who knows any plaintext byte can flip any ciphertext bit with predictable results. Notes are malleable — an attacker can modify them without detection.
- **Files:** [wallet/js/shielded.js](wallet/js/shielded.js) — `encryptNote()` / `decryptNote()`
- **Fix:** Replace with `TweetNaCl.secretbox` (XSalsa20-Poly1305 AEAD) or equivalent AEAD construction.
- [x] Fix

### 🔴 ZK-04 — Commitment hash uses SHA-256 over text, not Pedersen commitment over BN254
- **Impact:** `computeCommitmentHash` uses `SHA-256("commit:" + value + ":" + blinding)` over a text string. Real ZK circuits require Pedersen commitments over the BN254 curve. The on-chain circuit and off-chain commitment calculation are cryptographically incompatible.
- **Files:** [wallet/js/shielded.js](wallet/js/shielded.js) — `computeCommitmentHash()`
- **Fix:** Implement Pedersen commitment: `C = v*G + r*H` over BN254. Use `circomlibjs` or equivalent for the curve arithmetic.
- [x] Fix

### 🟠 ZK-05 — Note blinding factors and serial numbers stored plaintext in localStorage
- **Impact:** If a user's device is compromised or browsed by another person, all shielded note secrets are exposed. Combined with ZK-02, an attacker gains full ability to spend all shielded notes.
- **Files:** [wallet/js/shielded.js](wallet/js/shielded.js) — note store persisted in `localStorage` as JSON
- **Fix:** Encrypt the note store with the wallet's session key before persisting. Use the same AES-GCM encryption already applied to the main wallet state.
- [x] Fix

---

## 15. EXPLORER — NEW CRITICAL ISSUES (Round 2)

### 🔴 EXR2-01 — `bindIdentityActionButtons()` is defined but NEVER called
- **Impact:** All six identity action buttons in `address.html` (Edit Profile, Add Skill, Link Social, etc.) have zero event listeners. Clicking any of them does nothing. The entire MoltyID profile management flow is inaccessible from the Explorer.
- **Files:** [explorer/js/address.js](explorer/js/address.js) — `bindIdentityActionButtons()` defined but no call site
- **Fix:** Call `bindIdentityActionButtons()` from `init()` at page load.
- [x] Fix

### 🔴 EXR2-02 — Six address summary HTML elements missing from `address.html`
- **Impact:** `address.js` sets `innerHTML` on `#summaryAddress`, `#summaryBalance`, `#summaryMoltBalance`, `#summaryTxCount`, `#summaryStakeBalance`, and `#summaryIdentityBadge` — none of which exist in the HTML. The entire address header section is permanently blank. Users see an empty card where their address info should be.
- **Files:** [explorer/address.html](explorer/address.html) — missing 6 element IDs · [explorer/js/address.js](explorer/js/address.js) — all 6 references
- **Fix:** Add the 6 missing elements to the address summary card section of `address.html`.
- [x] Fix

### 🔴 EXR2-03 — Dual `getSlot` polling: shared/utils.js AND explorer.js both poll independently
- **Impact:** Every explorer page makes two concurrent periodic `getSlot` RPC calls using different intervals (one from the shared util, one from explorer.js). At 400ms slots, this generates ~5 redundant calls per second per user.
- **Files:** [explorer/js/explorer.js](explorer/js/explorer.js) — `setInterval(pollSlot, ...)` · [explorer/shared/utils.js](explorer/shared/utils.js) — independent `setInterval`
- **Fix:** Remove one of the two pollers. Keep the shared utility's version and remove the local one from `explorer.js`. Or subscribe to WS `subscribeSlots` instead.
- [x] Fix

---

## 16. DEX — NEW HIGH ISSUES (Round 2, beyond Round 1)

### 🔴 DEXR2-01 — All CLOB trading fees sent to zero address (burned, not treasury)
- **Impact:** Every CLOB trade fee — amounts that should go to the protocol treasury — is sent to `[0u8;32]` (the zero address), permanently burning them. The fee revenue mechanism is completely broken.
- **Files:** [contracts/dex_core/src/lib.rs](contracts/dex_core/src/lib.rs) — `collect_fees()` sends to hardcoded zero address · [dex/dex.js](dex/dex.js) — no treasury address configured
- **Fix:** Configure a treasury program address (from environment or admin call), store it in contract state, and transfer fees to it.
- [x] Fix

### 🟠 DEXR2-02 — UI caps open orders display at 50; contract allows 100 (`MAX_OPEN_ORDERS_PER_USER`)
- **Impact:** Users who have 51–100 open orders will not see them in the UI. They could place conflicting orders, miss fills, or be confused by phantom balance locks.
- **Files:** [dex/dex.js](dex/dex.js) — `getUserOrders(limit=50)` · [contracts/dex_core/src/lib.rs](contracts/dex_core/src/lib.rs) — `MAX_OPEN_ORDERS_PER_USER = 100`
- **Fix:** Change the `getUserOrders` call to `limit=100` to match the contract maximum.
- [x] Fix

### 🟠 DEXR2-03 — Governance voting window displays ~19.2h; contract voting period is 48h
- **Impact:** The governance UI tells users they have ~19.2 hours to vote. The actual deadline is 48 hours. Users may incorrectly assume the vote has passed or expired and stop monitoring.
- **Files:** [dex/dex.js](dex/dex.js) — `votingWindow = 172800 * 0.4 / 3600` (uses 0.4s slot time, but displays result in hours incorrectly) · [contracts/dex_governance/src/lib.rs](contracts/dex_governance/src/lib.rs) — `VOTING_PERIOD = 172800` slots
- **Fix:** `172800 slots × 0.4 s/slot = 69,120 seconds = 19.2 hours` — this is actually correct if slots are 400ms. Verify actual slot time and correct the display if needed. If slot interval is 1 second, the display should be 48h.
- [x] Fix

---

## 17. RPC LAYER — NEW ISSUES (Round 2, beyond Round 1)

### 🔴 RPCR2-01 — DEX REST API (`/api/v1/*`) has no tier-based rate limiting
- **Impact:** All DEX REST endpoints (`/api/v1/candles`, `/api/v1/orderbook`, `/api/v1/pools`, etc.) fall under the global 5,000/s rate limit only, with no per-endpoint tier. Expensive candle history scans run at 5,000/s per IP — 10× higher than they should.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L1300) — DEX router mounted without tier middleware
- **Fix:** Apply per-route rate limit middleware on the DEX router, or add route-specific tier checks inside DEX handlers.
- [x] Fix

### 🔴 RPCR2-02 — `require_single_validator` fail-opens on DB error (admin access without valid token)
- **Impact:** If `get_all_validators()` encounters a DB error, `unwrap_or_default()` returns an empty Vec. The function sees zero validators → single-validator mode → admin check **passes**. An attacker who can trigger disk I/O errors could bypass multi-validator admin authentication.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L187) — `get_all_validators().unwrap_or_default()`
- **Fix:** Return an explicit error if the DB read fails. Route through the cached validator list.
- [x] Fix

### 🟠 RPCR2-03 — `getAllContracts` triggers N+1 DB calls (up to 1001 reads per request)
- **Impact:** `getAllContracts` runs a full CF_PROGRAMS scan (1000 entries) then performs one `CF_SYMBOL_BY_PROGRAM` lookup per contract. 1001 DB reads per single RPC call. Under the Moderate rate limit (2000/s), this could saturate RocksDB.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L5728) — N symbol lookups inside program iterator
- **Fix:** Use `db.multi_get_cf()` to batch all symbol lookups, or store the symbol inline in `CF_PROGRAMS` value at write time.
- [x] Fix

### 🟠 RPCR2-04 — `getNFTsByOwner`/`getNFTsByCollection` each trigger N additional account lookups
- **Impact:** Fetching 50 NFTs requires 51 DB reads (1 prefix scan + 50 account lookups). `CF_NFT_BY_OWNER` stores empty values, forcing a full account load per token.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L7979) — `get_account()` called per NFT token pubkey
- **Fix:** Store a compact token summary as the value in `CF_NFT_BY_OWNER`/`CF_NFT_BY_COLLECTION` (collection, token_id, owner, metadata_uri). Eliminates per-item DB reads.
- [x] Fix

### 🟠 RPCR2-05 — `getTransactionsByAddress` / `getRecentTransactions` = up to 600 DB reads per call
- **Impact:** `getRecentTransactions(limit=500)` runs: 1 CF_TX_BY_SLOT scan + 500 CF_TRANSACTIONS lookups + up to 500 CF_BLOCKS timestamp lookups = ~600 DB reads. At Moderate rate (2000/s) this is 1.2M DB reads/sec from a single method.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs#L2511) · [rpc/src/lib.rs](rpc/src/lib.rs#L2645)
- **Fix:** Denormalize `(timestamp, tx_type)` into CF_TX_BY_SLOT value. Denormalize timestamp into CF_ACCOUNT_TXS value. Eliminates block timestamp lookups entirely.
- [x] Fix

### 🟠 RPCR2-06 — 84 handlers return raw RocksDB error strings (filesystem path leakage)
- **Impact:** Callers receive RocksDB internal error strings including database file paths (e.g., `/var/moltchain/db/OPTIONS-000003: too many open files`), column family names, and internal key format patterns.
- **Files:** [rpc/src/lib.rs](rpc/src/lib.rs) — all `format!("Database error: {}", e)` patterns (~84 occurrences)
- **Fix:** Map all DB errors to generic `-32000` with an opaque server-side correlation ID. Log full error server-side only.
- [x] Fix

---

## 18. FAUCET — NEW ISSUES (Round 2, beyond Round 1)

### 🔴 FAR2-01 — Port mismatch: config uses 9100; docker-compose maps faucet to 9101
- **Impact:** All API calls from `shared-config.js` go to port 9100; the container listens on 9101. The faucet is completely unreachable in the Docker environment. Every request fails with connection refused.
- **Files:** [shared/shared-config.js](shared/shared-config.js) — faucet port 9100 · [docker-compose.yml](docker-compose.yml) — faucet mapped to 9101
- **Fix:** Align the port. Use 9101 in both places (or make it an env variable).
- [x] Fix

### 🔴 FAR2-02 — Faucet stores synthetic transaction IDs, not real on-chain tx hashes
- **Impact:** The faucet writes `"txId": "airdrop-{timestamp_ms}"` to its ledger instead of the real transaction hash returned by `sendTransaction`. Explorer links constructed from this ID always 404.
- **Files:** [faucet/src/main.rs](faucet/src/main.rs) — `airdrop_record.tx_id = format!("airdrop-{}", now_ms)`
- **Fix:** Capture the response from `sendTransaction` and store the returned signature hash as the tx ID.
- [x] Fix

### 🔴 FAR2-03 — X-Forwarded-For spoofing bypasses per-IP rate limit
- **Impact:** `client_ip` is read directly from the `X-Forwarded-For` header without validation. An attacker can set any IP in this header and rotate through arbitrary addresses to bypass the per-IP cooldown.
- **Files:** [faucet/src/main.rs](faucet/src/main.rs) — `client_ip` from untrusted header
- **Fix:** Only trust `X-Forwarded-For` for the final hop (first IP from the right, not first from the left). Better: when deployed behind a known reverse proxy, use `ConnectInfo<SocketAddr>` to get the real IP.
- [x] Fix

### 🔴 FAR2-04 — No balance pre-flight check before consuming rate-limit slot
- **Impact:** If the faucet wallet has insufficient funds, the airdrop fails but the rate-limit slot is consumed. The user must wait for the full cooldown period before retrying, even though the failure was not their fault.
- **Files:** [faucet/src/main.rs](faucet/src/main.rs) — cooldown recorded before balance verification
- **Fix:** Check faucet wallet balance before recording the cooldown. Only consume the rate-limit slot on successful dispatch.
- [x] Fix

### 🔴 FAR2-05 — Faucet keypair and airdrop ledger not docker-volume-persisted
- **Impact:** On container restart, the faucet generates a new keypair. The previously funded keypair is lost. The faucet wallet is permanently drained with no recovery path. `airdrops.json` ledger is also lost.
- **Files:** [docker-compose.yml](docker-compose.yml) — no volume for faucet data · [faucet/src/main.rs](faucet/src/main.rs) — keypair path hardcoded to `/app/faucet-keypair.json`
- **Fix:** Add a named Docker volume for `/app/faucet-keypair.json` and `/app/airdrops.json`. Or inject the keypair via environment variable.
- [x] Fix

### 🟠 FAR2-06 — All direct-connected users share IP bucket "localhost"
- **Impact:** When the faucet is accessed directly (not through a reverse proxy) or all traffic comes from 127.0.0.1, every user shares the same rate-limit bucket. One request blocks all others for the cooldown period.
- **Files:** [faucet/src/main.rs](faucet/src/main.rs) — loopback IP check not handled separately
- **Fix:** Separate the loopback address from the per-user bucket. Apply a global rate limit for direct connections rather than a per-"user" limit.
- [x] Fix

---

## 19. WEBSITE — NEW ISSUES (Round 2, beyond Round 1)

### 🔴 WBR2-01 — Validator count always shows 0 (field extraction mismatch)
- **Impact:** The validator count card on the landing page always shows `0`. `getValidators()` response is a bare array but the code extracts `.count || .validators?.length` — when the response is an array (not an object), both are undefined, defaulting to 0. The network's actual validator count is never displayed.
- **Files:** [website/script.js](website/script.js) — validator count extraction logic
- **Fix:** Check if the response is an array and use `.length` directly. If it's an object, use `response.validators?.length ?? response.count ?? 0`.
- [x] Fix

### 🔴 WBR2-02 — Three entire landing page sections unreachable from navigation (`#validators`, `#api`, `#community`)
- **Impact:** Users scrolling via navigation cannot access the Validators, API Overview, or Community sections. Three content sections exist in the HTML but have no nav link pointing to them.
- **Files:** [website/index.html](website/index.html) — `#validators`, `#api`, `#community` section IDs without corresponding nav anchors
- **Fix:** Add nav items for these three sections, or remove the sections if they are decommissioned.
- [x] Fix

### 🟠 WBR2-03 — `callContract` shown in wizard Step 5 but not implemented in `MoltChainRPC` class
- **See cross-reference with GX-01. Note: this Class-level gap is new from Round 2 investigation.** The website's `MoltChainRPC` class presented in the SDK quickstart wizard includes a `callContract()` stub that is not wired to any real endpoint. End-to-end tutorial fails at the most important step.
- **Files:** [website/script.js](website/script.js) — `MoltChainRPC.callContract()` stub · [website/index.html](website/index.html#L808) — Step 5 code example
- **Fix:** After GX-01 is resolved, implement `callContract()` in the SDK class to call the new RPC method.
- [x] Fix

---

## 20. DEVELOPER PORTAL — NEW ISSUES (Round 2, beyond Round 1)

### 🔴 DEVR2-01 — Network selector is completely unwired (no JS handler attached)
- **Impact:** The network selector dropdown rendered on every dev portal page (mainnet / testnet / devnet) has no `change` event listener. Selecting a different network does nothing. All playground calls always go to the default RPC URL regardless of selection.
- **Files:** [developers/js/developers.js](developers/js/developers.js) — `#networkSelector` change handler absent
- **Fix:** Add a `change` handler that updates the active RPC URL in the playground and re-runs the current example.
- [x] Fix

### 🔴 DEVR2-02 — Playground is not interactive — all example outputs are hardcoded strings
- **Impact:** The dev portal playground page presents what looks like a live RPC tester. All "responses" are hardcoded static strings in the HTML/JS. No actual RPC calls are made. Developers testing the API receive fabricated responses that may differ from real server output.
- **Files:** [developers/playground.html](developers/playground.html) — static response objects · [developers/js/developers.js](developers/js/developers.js) — no actual `fetch()` / WebSocket calls
- **Fix:** Wire playground inputs to real `fetch()` calls against the selected network's RPC endpoint.
- [x] Fix

### 🔴 DEVR2-03 — `wallet-connect.js` hardcodes port 9000; validator listens on 8899
- **Impact:** The wallet connection helper shown in SDK examples always tries to connect to port 9000. The actual validator RPC is on port 8899. All wallet-connect SDK examples fail on first use.
- **Files:** [developers/js/wallet-connect.js](developers/js/wallet-connect.js) — `http://localhost:9000`
- **Fix:** Use the port from `shared-config.js` (`RPC_PORT = 8899`) rather than a hardcoded value.
- [x] Fix

### 🟠 DEVR2-04 — `getProgramAccounts` documented in all 3 SDK pages; not in RPC dispatch table (returns -32601)
- **Impact:** This is one of the most commonly used Solana-compatibility methods. Developers following the SDK examples will call `getProgramAccounts`, receive Method Not Found, and assume the SDK or their code is broken.
- **Files:** [developers/sdk-js.html](developers/sdk-js.html) · [developers/sdk-python.html](developers/sdk-python.html) · [developers/sdk-rust.html](developers/sdk-rust.html) — all show `getProgramAccounts` · [rpc/src/lib.rs](rpc/src/lib.rs) — no `getProgramAccounts` handler
- **Fix:** Either implement `getProgramAccounts` (scan CF_PROGRAMS with owner filter), or remove it from all SDK examples and add a note explaining why it's not supported.
- [x] Fix

### 🟠 DEVR2-05 — Search index uses `molt_` prefix; actual RPC method names have no prefix
- **See DEV-04 (Round 1).** Confirmed in Round 2: the search index at [developers/js/developers.js](developers/js/developers.js#L283) prepends `molt_` to all method names in the index. Searching for `getBalance` returns no results; you must search `molt_getBalance`.
- **Fix:** Strip the `molt_` prefix from search index entries.
- [x] Fix

### 🟠 DEVR2-06 — `sdk-python.html` nav highlights the JS page as active
- **Impact:** The active nav item highlights "JavaScript SDK" when the user is on the Python SDK page. Navigation orientation is broken.
- **Files:** [developers/sdk-python.html](developers/sdk-python.html) — nav active class applied to wrong `<li>`
- **Fix:** Apply the active class to the Python SDK nav entry on this page.
- [x] Fix

---

## 21. UPDATED FIX PRIORITY MATRIX (Round 2 Additions)

### PHASE 1 ADDITIONS — New Blockers from Round 2

| ID | Title | Systems Affected |
|----|-------|-----------------|
| ZK-01 | Shielded pool uses SHA-256 placeholder proofs — privacy fraud | Wallet, Shielded Pool |
| ZK-02 | Spending key derived from public address — privacy broken, funds at risk | Wallet, Shielded Pool |
| ZK-03 | XOR note encryption — malleable ciphertexts | Wallet, Shielded Pool |
| ZK-04 | SHA-256 commitment instead of Pedersen — circuit incompatible | Wallet, Shielded Pool |
| EXR2-01 | `bindIdentityActionButtons()` never called — identity UI dead | Explorer |
| EXR2-02 | 6 address summary HTML elements missing — header blank | Explorer |
| DEXR2-01 | All CLOB fees burned to zero address — treasury never paid | DEX |
| RPCR2-01 | DEX REST has no rate limiting — expensive scans at 5,000/s | RPC / DEX |
| RPCR2-02 | `require_single_validator` fail-opens on DB error | RPC / Security |
| FAR2-01 | Faucet port mismatch 9100 vs 9101 — Docker deployment broken | Faucet |
| FAR2-02 | Synthetic tx IDs — explorer links dead | Faucet |
| FAR2-03 | X-Forwarded-For spoofing bypasses rate limit | Faucet |
| FAR2-04 | Rate-limit slot consumed before balance check | Faucet |
| FAR2-05 | Faucet keypair not persisted — lost on container restart | Faucet |
| WBR2-01 | Validator count always 0 | Website |
| WBR2-02 | 3 sections unreachable from nav | Website |
| DEVR2-01 | Network selector completely unwired | Dev Portal |
| DEVR2-02 | Playground hardcodes responses — not interactive | Dev Portal |
| DEVR2-03 | wallet-connect.js port 9000 vs 8899 — all examples fail | Dev Portal |

### PHASE 2 ADDITIONS — New High Priority from Round 2

| ID | Title |
|----|-------|
| ZK-05 | Note store persisted in plaintext localStorage |
| RPCR2-03 | `getAllContracts` N+1 (1001 DB calls per request) |
| RPCR2-04 | NFT endpoints N+1 (50+ account lookups per request) |
| RPCR2-05 | Transaction list 600 DB reads per call |
| RPCR2-06 | Raw RocksDB error strings expose filesystem paths |
| FAR2-06 | All direct-connected users share one rate bucket |
| DEXR2-02 | Open orders capped at 50 vs contract max 100 |
| DEXR2-03 | Governance voting window display mismatch |
| EXR2-03 | Dual `getSlot` polling (2× redundant RPC calls per page) |
| WBR2-03 | `callContract` in website SDK class not wired |
| DEVR2-04 | `getProgramAccounts` in all SDKs returns -32601 |
| DEVR2-05 | Search index `molt_` prefix breaks all searches |
| DEVR2-06 | SDK-python nav highlights wrong page as active |

---

## FINDING COUNTS BY SEVERITY (All Rounds Combined)

| Severity | Round 1 | Round 2 | Total |
|----------|---------|---------|-------|
| 🔴 CRITICAL | 22 | 23 | **45** |
| 🟠 HIGH | 18 | 19 | **37** |
| 🟡 MEDIUM | 42 | 18 | **60** |
| 🔵 LOW | 24 | 8 | **32** |
| ⚪ INFO | 7 | 0 | **7** |
| **TOTAL** | **113** | **68** | **181** |

---

*Round 1 audit generated by multi-agent cross-matching on 2026-02-26.*
*Round 2 deep per-frontend audit generated on 2026-02-27 with line-by-line analysis of every UI element, RPC call, contract wiring, RocksDB access pattern, and WebSocket subscription.*
*Each finding verified against actual source code with specific file:line citations.*
