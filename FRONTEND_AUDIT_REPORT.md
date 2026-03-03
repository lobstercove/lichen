# Frontend Production-Readiness Audit Report

**Scope**: All 7 MoltChain frontends — Explorer, Wallet, Marketplace, Faucet, Website, DEX, Developers  
**Date**: 2026-02-25  
**Files audited**: 30+ JavaScript files, all HTML entry points, shared utilities  

---

## Executive Summary

| Severity | Count |
|----------|-------|
| Critical | 2 |
| High     | 4 |
| Medium   | 7 |
| Low      | 6 |
| **Total** | **19** |

The **Explorer**, **Faucet**, **Developers**, and **Marketplace** frontends are production-ready with only minor console.log cleanup needed. The **Wallet** has a critical hardcoded mock-prices issue and inconsistent `alert()` usage. The **DEX** has hardcoded fallback contract addresses and genesis price constants. The **Website** has cosmetic console.log branding.

---

## Finding #1 — Wallet: Hardcoded MOCK_PRICES for USD Valuations

| Field | Value |
|-------|-------|
| **Frontend** | Wallet |
| **File** | `wallet/js/wallet.js` |
| **Lines** | 13, 1307–1310, 1365, 1376, 1394 |
| **Category** | Mock/Fake Data, Hardcoded Values |
| **Severity** | **CRITICAL** |

**Code (line 13):**
```js
const MOCK_PRICES = { MOLT: 0.10, mUSD: 1.0, wSOL: 150.0, wETH: 3000.0, wBNB: 600.0 };
```

**Code (lines 1307–1310, in `refreshBalance()`):**
```js
// Calculate total USD value (using mock prices)
let totalUsd = molt * MOCK_PRICES.MOLT;
for (const [symbol, bal] of Object.entries(tokenBalances)) {
    totalUsd += bal * (MOCK_PRICES[symbol] || 0);
}
```

**Code (line 1365, in `loadAssets()`):**
```js
// Mock prices for display (using module-level MOCK_PRICES)
```

**Code (lines 1376, 1394):**
```js
const moltUsd = molt * MOCK_PRICES.MOLT;
// ...
const usdVal = bal * (MOCK_PRICES[symbol] || 0);
```

**Impact**: Every wallet user sees portfolio values computed from static fake prices. MOLT is always $0.10, wSOL always $150, wETH always $3,000, wBNB always $600. These values never update, giving users a false sense of their portfolio value. No price feed, oracle, or API integration exists in the wallet.

**Recommendation**: Integrate with MoltOracle contract (already deployed) or the DEX's Binance price feed to fetch live prices. Fall back to MOCK_PRICES only when live prices are unavailable, with a visible "estimated" badge.

---

## Finding #2 — DEX: Hardcoded Fallback Contract Addresses

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Lines** | 1044–1056 |
| **Category** | Hardcoded Data, Fragile Fallback |
| **Severity** | **CRITICAL** |

**Code (lines 1044–1056):**
```js
// Fallback: genesis-deployed addresses (deterministic from deployer + WASM)
// WARNING: These MUST match the live genesis auto-deploy. If contracts are
// recompiled, addresses change. Always prefer the symbol registry (above).
const needsFallback = !contracts.dex_core;
if (!contracts.dex_core) contracts.dex_core = '7QvQ1dxFTdSk9aSzbBe2gHCJH1bSRBDwVdPTn9M5iCds';
if (!contracts.dex_amm) contracts.dex_amm = '72AvbSmnkv82Bsci9BHAufeAGMTycKQX5Y6DL9ghTHay';
if (!contracts.dex_router) contracts.dex_router = 'FwAxYo2bKmCe1c5gZZjvuyopJMDgm1T9CAWr2svB1GPf';
if (!contracts.dex_margin) contracts.dex_margin = '8rTFuvbHZY89c3d9NktefAbHfjRoYh3vYJoC7eVgcw3W';
if (!contracts.dex_rewards) contracts.dex_rewards = '2okkNYSYPdN1jvhnhpXTmseFdXzgAgQXSCkQhgCkNiqC';
if (!contracts.dex_governance) contracts.dex_governance = '7BKw55h387pVAUs1dNApn2rfARBcGnnncXyb4WZDdGru';
if (!contracts.dex_analytics) contracts.dex_analytics = 'FBE25S5yGHUa6q38P8SjVXviw6dkoqD7oCMUuxj1aRof';
if (!contracts.prediction_market) contracts.prediction_market = 'J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD';
```

**Impact**: If the symbol registry RPC call fails (network issue, node restart), the DEX silently uses hardcoded addresses. These will cause **transaction failures and potential fund loss** if contracts were ever recompiled (changing their addresses). The warning log is only visible in the browser console.

**Recommendation**: Show a visible UI banner when fallback addresses are active ("DEX running in degraded mode — some transactions may fail"). Block trading until registry is confirmed, or periodically retry the registry lookup.

---

## Finding #3 — Wallet: Inconsistent `alert()` Usage (17 instances)

| Field | Value |
|-------|-------|
| **Frontend** | Wallet |
| **File** | `wallet/js/wallet.js` |
| **Lines** | 623, 640, 690, 695, 826, 970, 978, 984, 1022, 1028, 1033, 1067, 1077, 1100, 1126, 3030, 3035 |
| **Category** | UX / Missing Error Handling |
| **Severity** | **HIGH** |

**Examples:**
```js
// Line 623
alert('Please enter password');

// Line 640
alert('Incorrect password');

// Line 690
alert('Password must be at least 8 characters');

// Line 826
alert('Words are in wrong order. Try again!');

// Line 3030
alert('Invalid recipient address');
```

**Impact**: The wallet uses `showToast()` for 98 messages but falls back to blocking `alert()` for 17 critical validation errors. `alert()` blocks the JavaScript event loop, is not styleable, breaks UX consistency, and on some mobile browsers can escape the viewport.

**Recommendation**: Replace all 17 `alert()` calls with the existing `showToast()` function (already used elsewhere in the same file). For destructive confirmations (wrong seed order), use a styled modal instead.

---

## Finding #4 — Marketplace: `alert()` for Validation Messages

| Field | Value |
|-------|-------|
| **Frontend** | Marketplace |
| **Files** | `marketplace/js/marketplace.js`, `marketplace/js/create.js` |
| **Lines** | marketplace.js:258, create.js: multiple |
| **Category** | UX / Inconsistent Error Handling |
| **Severity** | **HIGH** |

**Code (marketplace.js line 258):**
```js
alert('Please connect your wallet first');
```

**Code (create.js, multiple lines):**
```js
alert('Unsupported file type. Upload image, video, or audio.');
alert('File too large. Maximum 50MB.');
alert('Please connect your wallet first');
alert('Please enter an NFT name');
alert('NFT name must be 128 characters or fewer');
alert('Description must be 2048 characters or fewer');
alert('Please upload an image or media file');
alert('Supply must be 1–1000');
alert('Royalty must be 0–10%');
alert('Please select a collection or create a new one');
alert('Please enter a name for your new collection');
alert('Insufficient balance...');
alert('Wallet signing unavailable...');
```

**Impact**: Same as Finding #3. The marketplace has `showToast()` defined in all page files but uses `alert()` for validation. This creates an inconsistent UX — some errors show toasts, others show browser alerts.

**Recommendation**: Replace all `alert()` calls with `showToast(msg, 'error')`.

---

## Finding #5 — DEX: Hardcoded Genesis Price Constant

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Lines** | 965, 1001, 1199–1202 |
| **Category** | Hardcoded Data |
| **Severity** | **HIGH** |

**Code (line 965):**
```js
const MOLT_GENESIS_PRICE = 0.10;
```

**Code (line 1001):**
```js
lastPrice: MOLT_GENESIS_PRICE,
```

**Code (lines 1199–1202, in `loadPairsFromAPI()`):**
```js
pairs = [{
    pairId: 1, id: 'MOLT/mUSD', base: 'MOLT', quote: 'mUSD',
    price: MOLT_GENESIS_PRICE, change: 0, ...
}];
console.info('[DEX] No trading pairs on-chain — using genesis MOLT/mUSD @ $0.10');
```

**Impact**: When no on-chain trading pairs exist (fresh deployment, API failure), the DEX creates a synthetic MOLT/mUSD pair at a hardcoded $0.10. Users may see stale prices, and the `lastPrice` state initializes to 0.10 regardless of actual market conditions.

**Recommendation**: This is acceptable as genesis bootstrapping, but add a visible "Genesis Price" indicator on the UI when using this fallback, and disable actual trading against synthetic pairs.

---

## Finding #6 — DEX: Binance WebSocket External Dependency

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Lines** | 3408–3442 |
| **Category** | External Dependency / Security |
| **Severity** | **HIGH** |

**Code (lines 3418–3440):**
```js
function connectBinancePriceFeed() {
    const streams = 'solusdt@miniTicker/ethusdt@miniTicker/bnbusdt@miniTicker';
    const url = `wss://stream.binance.com:9443/ws/${streams}`;
    try {
        binanceWs = new WebSocket(url);
        binanceWs.onmessage = (evt) => {
            // ... parse price and overlay on DEX pairs
        };
        console.log('[DEX] Binance price feed connected (real-time overlay)');
    } catch (e) {
        console.warn('[DEX] Binance price feed unavailable:', e.message);
    }
}
```

**Impact**: When `ENABLE_EXTERNAL_PRICE_WS` is enabled (opt-in via localStorage), the browser makes a direct WebSocket to Binance's public API. This creates: (1) an external dependency on Binance infrastructure, (2) privacy leakage of user's IP to Binance, (3) potential price manipulation if the WS is spoofed via DNS hijack. The feature is opt-in (default off on line 6862), which mitigates the severity.

**Recommendation**: Route external price data through the MoltChain backend oracle instead of a direct browser-to-Binance connection. The backend oracle price feeder already exists (mentioned in comments).

---

## Finding #7 — Website: Branded console.log Statements

| Field | Value |
|-------|-------|
| **Frontend** | Website |
| **File** | `website/script.js` |
| **Lines** | ~565–582 |
| **Category** | Console.log Statements |
| **Severity** | **MEDIUM** |

**Code:**
```js
console.log('%c🦞 MoltChain', 'font-size: 24px; font-weight: bold; color: #FF6B35;');
console.log('%cThe Agent-First Blockchain', 'font-size: 14px; color: #B8C1EC;');
console.log('%cWebsite loaded successfully', 'font-size: 12px; color: #06D6A0;');
console.log('%cRPC URL:', 'font-size: 12px; color: #6B7A99;', getRpcEndpoint());
```

**Impact**: Prints branded ASCII art and the RPC URL to every user's browser console. The RPC URL disclosure is a minor information leak.

**Recommendation**: Remove or gate behind a debug flag. The RPC URL should not be logged to the console.

---

## Finding #8 — Website: WebSocket console.log

| Field | Value |
|-------|-------|
| **Frontend** | Website |
| **File** | `website/script.js` |
| **Lines** | ~283, ~293 |
| **Category** | Console.log Statements |
| **Severity** | **MEDIUM** |

**Code:**
```js
console.log('[WS] Connected');
console.log('[WS] Disconnected, reconnecting in 5s');
```

**Impact**: WebSocket lifecycle events logged to console on every connect/disconnect cycle. Minor noise in production.

**Recommendation**: Remove or reduce to `console.debug()`.

---

## Finding #9 — Marketplace: "Loading/Ready" console.log on Every Page

| Field | Value |
|-------|-------|
| **Frontend** | Marketplace |
| **Files** | `marketplace/js/marketplace.js`, `browse.js`, `item.js`, `profile.js`, `create.js`, `marketplace-data.js` |
| **Lines** | Various (DOMContentLoaded handlers) |
| **Category** | Console.log Statements |
| **Severity** | **MEDIUM** |

**Examples (10 total):**
```js
console.log('Molt Market loading...');       // marketplace.js
console.log('Molt Market ready');            // marketplace.js
console.log('Molt Market Browse loading...'); // browse.js
console.log('Molt Market Browse ready');     // browse.js
console.log('Molt Market Item loading...');  // item.js
console.log('Molt Market Item ready');       // item.js
console.log('Molt Market Profile loading...');// profile.js
console.log('Molt Market Profile ready');    // profile.js
console.log('Molt Market Create loading...'); // create.js
console.log('Molt Market Create ready');     // create.js
console.log('Molt Market data source initialized (RPC-backed, zero mock data)'); // marketplace-data.js
```

**Impact**: Every marketplace page load prints 3-4 log messages. Noise in production console.

**Recommendation**: Remove all "loading/ready" logs. Keep `console.warn`/`console.error` for error conditions.

---

## Finding #10 — DEX: console.log for Operational Events

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Lines** | 97, 1037, 3440 |
| **Category** | Console.log Statements |
| **Severity** | **MEDIUM** |

**Code:**
```js
console.log('[WS] Connected');                                      // line 97
console.log('[DEX] Contract addresses loaded from symbol registry'); // line 1037
console.log('[DEX] Binance price feed connected (real-time overlay)'); // line 3440
```

**Impact**: Operational lifecycle events visible in production console. Minor information disclosure.

**Recommendation**: Remove or gate behind `DEV_MODE` flag.

---

## Finding #11 — Website: DOMContentLoaded console.log

| Field | Value |
|-------|-------|
| **Frontend** | Website |
| **File** | `website/script.js` |
| **Line** | ~555 |
| **Category** | Console.log Statements |
| **Severity** | **MEDIUM** |

**Code:**
```js
console.log('MoltChain website loaded 🦞');
```

**Impact**: Minor. Emoji in production console output.

**Recommendation**: Remove.

---

## Finding #12 — Explorer: Single Commented-Out console.log (Residual)

| Field | Value |
|-------|-------|
| **Frontend** | Explorer |
| **File** | `explorer/js/explorer.js` |
| **Category** | Console.log Statements |
| **Severity** | **LOW** |

**Code:**
```js
// console.log('🦞 Reef Explorer loaded');
```

**Impact**: Properly commented out. No runtime impact. Mentioned only for completeness.

**Recommendation**: Delete the commented line in a cleanup pass.

---

## Finding #13 — Wallet: Commented-Out Debug Logs (Residual)

| Field | Value |
|-------|-------|
| **Frontend** | Wallet |
| **File** | `wallet/js/wallet.js` |
| **Lines** | Various |
| **Category** | Dead Code |
| **Severity** | **LOW** |

**Code (multiple locations):**
```js
// console.log('[Bridge] Lock event for our wallet:', data);
// console.log('[Bridge] Mint event for our wallet:', data);
// console.log('Token registry loaded from manifest');
// console.log('Token registry loaded from localStorage');
// console.log('MoltWallet loaded');
// console.log('EVM address registered:', evmAddress, '→', wallet.address);
// console.log('MoltWallet fully initialized');
```

**Impact**: No runtime impact (all commented out). The wallet team properly disabled debug logs. These are residual cleanup items.

**Recommendation**: Delete in a cleanup pass.

---

## Finding #14 — DEX: Info-Level Log for Genesis Pair Fallback

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Line** | 1202 |
| **Category** | Console.log Statements |
| **Severity** | **LOW** |

**Code:**
```js
console.info('[DEX] No trading pairs on-chain — using genesis MOLT/mUSD @ $0.10');
```

**Impact**: Informational log for a significant state (no trading pairs). Acceptable as operational logging, but should not persist in long-running production.

**Recommendation**: Keep during early mainnet phase, remove after stabilization.

---

## Finding #15 — Wallet: `crypto.js` bs58 Fallback Warning

| Field | Value |
|-------|-------|
| **Frontend** | Wallet |
| **File** | `wallet/js/crypto.js` |
| **Category** | Missing Dependency Warning |
| **Severity** | **LOW** |

**Code:**
```js
console.warn('bs58 not loaded, using hex address');
```

**Impact**: If the base58 library fails to load, the wallet falls back to hex addresses and warns. This is proper graceful degradation with a warning.

**Recommendation**: No action needed. This is correct error handling.

---

## Finding #16 — DEX: GOVERNANCE_MIN_QUORUM_DEFAULT = 3

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Line** | 967 |
| **Category** | Hardcoded Default |
| **Severity** | **LOW** |

**Code:**
```js
const GOVERNANCE_MIN_QUORUM_DEFAULT = 3;
```

**Impact**: Governance quorum defaults to 3 if the protocol params API is unavailable. This is a reasonable conservative default, but should be documented.

**Recommendation**: No immediate action. Document in governance docs.

---

## Finding #17 — DEX: Fallback Warning is Console-Only

| Field | Value |
|-------|-------|
| **Frontend** | DEX |
| **File** | `dex/dex.js` |
| **Line** | 1056 |
| **Category** | Missing UI Warning |
| **Severity** | **MEDIUM** |

**Code:**
```js
console.warn('[DEX] ⚠️ FALLBACK ADDRESSES ACTIVE — symbol registry was unavailable. Transactions WILL fail if contracts were recompiled. Set MOLTCHAIN_RPC or check node connectivity.');
```

**Impact**: Critical operational warning is only visible in the browser console. No UI notification to the user that the DEX is running in degraded mode.

**Recommendation**: Add a visible banner/toast when fallback addresses are active.

---

## Finding #18 — DEX: MINTING_FEE Hardcoded in create.js

| Field | Value |
|-------|-------|
| **Frontend** | Marketplace |
| **File** | `marketplace/js/create.js` |
| **Line** | 15 |
| **Category** | Hardcoded Value |
| **Severity** | **MEDIUM** |

**Code:**
```js
var MINTING_FEE = 0.5;
```

**Impact**: The NFT minting fee is hardcoded at 0.5 MOLT. If the on-chain fee changes (via governance), the UI will show incorrect cost estimates.

**Recommendation**: Fetch the minting fee from the marketplace contract or protocol params API.

---

## Finding #19 — Marketplace create.js: Collection Deployment Fee Hardcoded

| Field | Value |
|-------|-------|
| **Frontend** | Marketplace |
| **File** | `marketplace/js/create.js` |
| **Lines** | ~345 (inside `updatePriceBreakdown()`) |
| **Category** | Hardcoded Value |
| **Severity** | **LOW** |

**Code:**
```js
var collectionFee = isNewCollection ? 1.0 : 0;
```

**Impact**: Collection deployment cost is displayed as 1.0 MOLT regardless of actual on-chain fee. Minor since the actual transaction will succeed/fail based on real on-chain fees, but the preview is misleading.

**Recommendation**: Fetch from protocol params.

---

## Clean Frontends — No Issues Found

### Explorer (`explorer/js/`)
- **Files audited**: explorer.js, utils.js, blocks.js, block.js, transactions.js, transaction.js, address.js, validators.js, contract.js, contracts.js, agents.js, privacy.js
- **Status**: ✅ **Production-ready**
- All RPC calls use real JSON-RPC 2.0 integration
- All `console.log` statements are commented out
- `console.warn`/`console.error` used only for actual error conditions
- XSS protection via `escapeHtml()` throughout
- Proper WebSocket reconnection with exponential backoff
- No mock data, stubs, or placeholders detected

### Faucet (`faucet/faucet.js`)
- **Status**: ✅ **Production-ready**
- Real API integration with faucet backend (`/faucet/request`, `/faucet/config`, `/faucet/airdrops`)
- Proper input validation (base58 format, length check)
- Math captcha anti-abuse
- XSS escaping on all user input (`escapeHtml()`, `encodeURIComponent()`)
- Request timeout (15s) with abort controller
- No console.log statements
- No mock data

### Developers (`developers/js/developers.js`)
- **Status**: ✅ **Production-ready**
- Pure navigation/UI logic (sidebar, scroll spy, code copy, language tabs, search)
- Static search index pointing to real documentation pages
- No RPC calls, no mock data, no console.log
- Keyboard accessibility (Cmd+K search, arrow navigation)

### Marketplace Data Layer (`marketplace/js/marketplace-data.js`)
- **Status**: ✅ **Production-ready** (data layer)
- File header: "RPC-backed, zero mock data" — **verified true**
- All functions call real RPCs: `getMarketListings`, `getMarketSales`, `getNFTsByOwner`, `getNFT`, `getAllContracts`, etc.
- Returns empty arrays on RPC failure (graceful degradation)
- One initialization `console.log` (Finding #9)

---

## Summary of Recommendations by Priority

### Immediate (Pre-Mainnet)
1. **Replace `MOCK_PRICES` in wallet** with live price feed (Finding #1)
2. **Add UI banner for DEX fallback addresses** (Findings #2, #17)
3. **Replace all `alert()` with `showToast()`** in wallet (Finding #3) and marketplace (Finding #4)

### Short-Term
4. Remove all `console.log` statements from marketplace, website, and DEX (Findings #7-11)
5. Fetch minting/deployment fees from chain (Findings #18, #19)
6. Route Binance price feed through backend oracle (Finding #6)

### Cleanup
7. Delete commented-out console.log lines (Findings #12, #13)
8. Add "Genesis Price" UI indicator for DEX (Finding #5)
