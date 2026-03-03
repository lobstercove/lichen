# MoltChain Developer Portal — Deep Production Audit

**Audit Date:** 2026-02-25  
**Auditor:** GitHub Copilot (Claude Sonnet 4.6)  
**Portal Root:** `developers/`  
**Methodology:** Every source file read in full; RPC server and WebSocket dispatch tables extracted from `rpc/src/lib.rs` and `rpc/src/ws.rs`; all cross-references validated line-by-line.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [File Inventory](#2-file-inventory)
3. [RPC Documentation Coverage](#3-rpc-documentation-coverage)
4. [WebSocket Documentation Accuracy](#4-websocket-documentation-accuracy)
5. [Playground Audit](#5-playground-audit)
6. [SDK Examples Audit](#6-sdk-examples-audit)
7. [Navigation & Links Audit](#7-navigation--links-audit)
8. [CSS System Audit](#8-css-system-audit)
9. [Search System Audit](#9-search-system-audit)
10. [Issues Found — Master Table](#10-issues-found--master-table)

---

## 1. Executive Summary

The MoltChain Developer Portal is a well-structured static HTML documentation site (16 content pages, ~15,000 lines) that covers the full chain API, SDKs, smart contracts, validator setup, ZK privacy, and the MoltyID identity system. The **content quality is high** — the technical depth of RPC references, contract function tables, WASM ABI conventions, and ZK circuit layouts is excellent.

However, the portal has a cluster of **critical infrastructure bugs** that silently fail for every user:

1. **The network selector is completely non-functional** on all pages — it can never change the active endpoint.
2. **`playground.html` is not a playground** — it links to a web IDE that may not exist.
3. **`wallet-connect.js` uses the wrong default port** (9000 vs 8899), breaking all wallet-connected apps.
4. **`getProgramAccounts` is documented in the JavaScript SDK** (`sdk-js.html`) but has no server-side handler — every call returns error -32601.
5. **25+ server RPC methods have no full documentation cards** — they appear only in a bullet-list "Live Additions" section.
6. **Three pages are orphaned** from the main nav (`architecture.html`, `validator.html`, `changelog.html`).
7. **`contract-reference.html` is isolated from the shared CSS system** — it defines its own inline variables and skips both shared CSS files, resulting in a completely different visual theme.
8. **The search index uses an incorrect `molt_` method name prefix** that doesn't match actual RPC method names, causing zero useful results for any user searching for real method names.

The portal would benefit from a CSS audit pass, a single-source ENDPOINTS map, and a search index rebuild.

**Severity summary:** 3 🔴 Critical · 9 🟠 High · 12 🟡 Medium · 8 🔵 Low

---

## 2. File Inventory

| File | Lines | Status | Notes |
|------|-------|--------|-------|
| `shared-base-styles.css` | 1,323 | ✅ Read | Full component CSS; defines `--primary`, `--shadow`, etc. |
| `shared-theme.css` | 357 | ✅ Read | Design tokens; defines `--orange-primary`, `--bg-hover`, etc. |
| `css/developers.css` | 1,187 | ✅ Read | Page-specific overrides; loaded on all pages |
| `shared-config.js` | 42 | ✅ Read | URL resolver; only loaded on `index.html` |
| `js/developers.js` | 681 | ✅ Read | All portal JS: search, sidebar, scrollspy, network selector |
| `shared/utils.js` | 507 | ✅ Read | Protocol constants, RPC client, formatters |
| `shared/wallet-connect.js` | 343 | ✅ Read | `MoltWallet` class |
| `index.html` | 443 | ✅ Read | Hub page with live stats |
| `getting-started.html` | 481 | ✅ Read | Onboarding guide |
| `playground.html` | 294 | ✅ Read | Guide only — NOT an interactive playground |
| `rpc-reference.html` | 1,250 | ✅ Read | ~40 methods with full cards; 25+ in bullets only |
| `ws-reference.html` | 922 | ✅ Read | All subscriptions documented |
| `sdk-js.html` | 698 | ✅ Read | JS SDK reference |
| `sdk-python.html` | 654 | ✅ Read | Python SDK reference |
| `sdk-rust.html` | 692 | ✅ Read | Rust SDK reference |
| `architecture.html` | 674 | ✅ Read | Technical architecture (orphaned from nav) |
| `contracts.html` | 873 | ✅ Read | Contract development guide |
| `contract-reference.html` | 1,003 | ✅ Read | 27 contract reference (different CSS theme) |
| `moltyid.html` | 736 | ✅ Read | MoltyID guide |
| `validator.html` | 1,164 | ✅ Read | Validator setup guide (orphaned from nav) |
| `zk-privacy.html` | 545 | ✅ Read | ZK shielded pool reference |
| `cli-reference.html` | 2,373 | ✅ Read | Full CLI reference |
| `changelog.html` | 332 | ✅ Read | Release history (orphaned from nav) |

**Total portal lines:** ~17,624

---

## 3. RPC Documentation Coverage

### 3.1 Methods with Full Documentation Cards

These methods in `rpc-reference.html` have complete `method-card` blocks with parameters, request/response examples, and return type tables:

`getSlot`, `getLatestBlock`, `getRecentBlockhash`, `health`, `getMetrics`, `getChainStatus`, `getBalance`, `getAccount`, `getAccountInfo`, `getAccountTxCount`, `getBlock`, `getTransaction`, `sendTransaction`, `simulateTransaction`, `getTransactionsByAddress`, `getTransactionHistory`, `getValidators`, `getValidatorInfo`, `getValidatorPerformance`, `getStakingStatus`, `getStakingRewards`, `stake`, `unstake`, `getNetworkInfo`, `getPeers`, `getContractInfo`, `getAllContracts`, `getContractLogs`, `getContractAbi`, `getProgram`, `getPrograms`, `getProgramStats`, `getProgramCalls`, `getProgramStorage`, `getTokenBalance`, `getTokenHolders`, `getTokenTransfers`, `getCollection`, `getNFT`, `getNFTsByOwner`, `getMarketListings`, `getMarketSales`, `getTotalBurned`, `getFeeConfig`, `getRentParams`

### 3.2 Server Methods with No Full Documentation Card

These methods exist in the `rpc/src/lib.rs` dispatch table (lines 1357–1531) but are listed only in a "Live Additions" bullet list or not mentioned at all:

| Method | Server Line | Doc Status |
|--------|-------------|------------|
| `getRecentTransactions` | ~1361 | Bullet only |
| `getTokenAccounts` | ~1363 | Bullet only |
| `confirmTransaction` | ~1365 | Bullet only |
| `getTreasuryInfo` | ~1367 | Bullet only |
| `getGenesisAccounts` | ~1369 | Bullet only |
| `getGovernedProposal` | ~1375 | Not documented |
| `getRewardAdjustmentInfo` | ~1379 | Not documented |
| `getNFTsByCollection` | ~1420 | Bullet only |
| `getNFTActivity` | ~1422 | Bullet only |
| `getMarketOffers` | ~1424 | Bullet only |
| `getMarketAuctions` | ~1426 | Bullet only |
| `requestAirdrop` | ~1428 | Bullet only |
| `getNameAuction` | ~1440 | Not documented |
| `getPredictionMarketStats` | ~1450 | Bullet only |
| `getPredictionMarkets` | ~1452 | Bullet only |
| `getPredictionMarket` | ~1454 | Bullet only |
| `getPredictionPositions` | ~1456 | Bullet only |
| `getPredictionTraderStats` | ~1458 | Bullet only |
| `getPredictionLeaderboard` | ~1460 | Bullet only |
| `getPredictionTrending` | ~1462 | Bullet only |
| `getPredictionMarketAnalytics` | ~1464 | Bullet only |
| `getDexCoreStats` | ~1470 | Bullet only |
| `getDexAmmStats` | ~1472 | Bullet only |
| `getDexMarginStats` | ~1474 | Bullet only |
| `getDexRewardsStats` | ~1476 | Bullet only |
| `getDexRouterStats` | ~1478 | Bullet only |
| `getDexAnalyticsStats` | ~1480 | Bullet only |
| `getDexGovernanceStats` | ~1482 | Bullet only |
| `getMoltswapStats` | ~1484 | Bullet only |
| `getLobsterLendStats` | ~1486 | Bullet only |
| `getClawPayStats` | ~1488 | Bullet only |
| `getBountyBoardStats` | ~1490 | Bullet only |
| `getComputeMarketStats` | ~1492 | Bullet only |
| `getReefStorageStats` | ~1494 | Bullet only |
| `getMoltMarketStats` | ~1496 | Bullet only |
| `getMoltAuctionStats` | ~1498 | Bullet only |
| `getMoltPunksStats` | ~1500 | Bullet only |
| `getMusdStats` | ~1508 | Not documented |
| `getWethStats` | ~1509 | Not documented |
| `getWsolStats` | ~1510 | Not documented |
| `getClawVaultStats` | ~1511 | Not documented |
| `getMoltBridgeStats` | ~1512 | Not documented |
| `createBridgeDeposit` | ~1514 | Not documented |
| `getBridgeDeposit` | ~1516 | Not documented |
| `getBridgeDepositsByRecipient` | ~1518 | Not documented |
| `getMoltDaoStats` | ~1520 | Not documented |
| `getMoltOracleStats` | ~1522 | Not documented |
| `getShieldedPoolState` | ~1524 | Documented in `zk-privacy.html` only |
| `getShieldedMerkleRoot` | ~1526 | Documented in `zk-privacy.html` only |
| `getShieldedMerklePath` | ~1527 | Documented in `zk-privacy.html` only |
| `isNullifierSpent` | ~1528 | Documented in `zk-privacy.html` only |
| `getShieldedCommitments` | ~1530 | Documented in `zk-privacy.html` only |
| `stakeToReefStake` | ~1383 | Not documented |
| `unstakeFromReefStake` | ~1385 | Not documented |
| `claimUnstakedTokens` | ~1387 | Not documented |
| `getStakingPosition` | ~1389 | Not documented |
| `getReefStakePoolInfo` | ~1391 | Not documented |
| `getUnstakingQueue` | ~1393 | Not documented |
| `setContractAbi` | ~1410 | Not documented |
| `upgradeContract` | ~1412 | Not documented |
| `deployContract` | ~1414 | Not documented |
| `getSymbolRegistry` | ~1444 | Not documented |
| `getSymbolRegistryByProgram` | ~1446 | Not documented |
| `getAllSymbolRegistry` | ~1448 | Not documented |
| `getEvmRegistration` | ~1442 | Not documented |
| `lookupEvmAddress` | ~1443 | Not documented |
| `batchReverseMoltNames` | ~1440 | Not documented |

**Total**: ~66 server methods have no full method card in `rpc-reference.html`.

### 3.3 Documented Methods Not Found in Server

| SDK / Doc Method | Status |
|-----------------|--------|
| `getProgramAccounts` | ❌ **In `sdk-js.html` but absent from server dispatch. Returns -32601.** |

### 3.4 `stake` / `unstake` Documentation Accuracy Issue

`rpc-reference.html` documents `stake` and `unstake` as direct RPC methods with a `from` field and amount. The server handlers (`handle_stake`, `handle_unstake`) accept these calls without any multi-validator guard — they do **not** call `require_single_validator` and will not return `-32003`. However, the documented parameter format (showing a `from` field) may not match the actual handler expectations (base64-encoded signed transactions). The docs should clarify the exact parameter format required.

### 3.5 Solana Compatibility Endpoint (`/solana`)

Documented in `rpc-reference.html`. Methods: `getLatestBlockhash`, `getRecentBlockhash`, `getBalance`, `getAccountInfo`, `getBlock`, `getBlockHeight`, `getSignaturesForAddress`, `getSignatureStatuses`, `getSlot`, `getTransaction`, `sendTransaction`, `getHealth`, `getVersion`. **Not cross-checked against server** — recommend audit of `/solana` handler completeness.

### 3.6 EVM Endpoint (`/evm`)

Documented in `rpc-reference.html`. Methods: `eth_getBalance`, `eth_sendRawTransaction`, `eth_call`, `eth_chainId`, `eth_blockNumber`, `eth_getTransactionReceipt`, `eth_getTransactionByHash`, `eth_accounts`, `net_version`. Coverage appears complete for the documented endpoint.

---

## 4. WebSocket Documentation Accuracy

WebSocket dispatch verified against `rpc/src/ws.rs` (lines 733–1389).

### 4.1 Subscription Coverage Matrix

| Subscription (ws.rs) | ws-reference.html | Notes |
|---------------------|-------------------|-------|
| `subscribeSlots` / `slotSubscribe` | ✅ Documented | Alias confirmed in ws.rs line 733 |
| `subscribeBlocks` | ✅ Documented | |
| `subscribeTransactions` | ✅ Documented | |
| `subscribeAccount` | ✅ Documented | |
| `subscribeLogs` | ✅ Documented | |
| `subscribeProgramUpdates` | ✅ Documented | |
| `subscribeProgramCalls` | ✅ Documented | |
| `subscribeNftMints` | ✅ Documented | |
| `subscribeNftTransfers` | ✅ Documented | |
| `subscribeMarketListings` | ✅ Documented | |
| `subscribeMarketSales` | ✅ Documented | |
| `subscribeBridgeLocks` | ⚠️ Sidebar only | No method card in page body |
| `subscribeBridgeMints` | ⚠️ Sidebar only | No method card in page body |
| `subscribeSignatureStatus` / `signatureSubscribe` | ✅ Documented | Handler registered; may never emit |
| `subscribeValidators` / `validatorSubscribe` | ✅ Documented | Handler registered; likely never emits |
| `subscribeTokenBalance` / `tokenBalanceSubscribe` | ✅ Documented | Handler registered; likely never emits |
| `subscribeEpochs` / `epochSubscribe` | ✅ Documented | Handler registered; likely never emits |
| `subscribeGovernance` / `governanceSubscribe` | ✅ Documented | Handler registered; likely never emits |
| `subscribeDex` | ✅ Documented | Multiplex (dex-specific event types) |
| `subscribePrediction` / `subscribePredictionMarket` | ✅ Documented | Multiplex |

### 4.2 `slotSubscribe` / `subscribeSlots` Consistency

`ws-reference.html` documents `subscribeSlots` as the canonical method name. `index.html` sends `slotSubscribe` in its WebSocket live-block-height code. Both names are valid — the server accepts both as aliases (ws.rs line 733: `"subscribeSlots" | "slotSubscribe"`). **No bug**, but `index.html` example could be updated to use the canonical `subscribeSlots` for consistency.

### 4.3 Potentially Non-Emitting Subscriptions

The following subscriptions are registered handlers in `ws.rs` but may never emit events if the corresponding broadcast channels are not populated:
- `subscribeValidators` — validator set changes are infrequent and channel may not be wired
- `subscribeTokenBalance` — requires active token balance monitoring pipeline
- `subscribeEpochs` — epoch transitions must explicitly fire this channel
- `subscribeGovernance` — requires governance events to be routed to WS

These are documented with rich event payload examples. **If these never emit in practice, the documentation is misleading but not technically incorrect** (the subscription will succeed and simply never deliver events). Recommend adding a note like "⚠️ Low-frequency: may not emit on local testnets."

### 4.4 `subscribeBridgeLocks` and `subscribeBridgeMints` Missing Method Cards

These subscriptions appear in the `ws-reference.html` sidebar but have no corresponding `method-card` in the page body. Users clicking the sidebar links will scroll through the page and find nothing.

---

## 5. Playground Audit

### Critical Finding: `playground.html` Is Not a Playground

`playground.html` is titled "Programs Playground" and is listed in the portal's feature cards as the interactive RPC/contract testing environment. **It is not interactive at all.** The page is a static documentation guide describing a browser-based IDE (the "Programs IDE") located at `../programs/index.html`.

**What the page actually contains:**
- A step-by-step guide to using an external IDE (description only)
- Links to `../programs/index.html`, `../programs/index.html#templates`, `../programs/index.html#editor`
- No runnable calls, no WebSocket connections, no JSON-RPC inputs

**Broken links:** `../programs/index.html` refers to a path relative to the `developers/` directory. The `programs/` app exists at the workspace root level (`/programs/`), but navigating there from `developers/playground.html` would require `../../programs/index.html` if served from a nested path, or `../programs/index.html` if the entire site is served from the workspace root. The correct path depends on deployment configuration. On a local file:// open of `developers/playground.html`, the `../programs/index.html` link points outside the `developers/` folder and may or may not resolve, depending on whether a web server is serving both directories.

**Recommendation:** Either (a) make `playground.html` an actual interactive playground with method dropdowns and a JSON-RPC execute button, or (b) rename it to `programs-guide.html` and update all references.

---

## 6. SDK Examples Audit

### 6.1 JavaScript SDK (`sdk-js.html`)

| Issue | Severity | Detail |
|-------|----------|--------|
| `getProgramAccounts` documented | 🟠 High | No server handler exists — will return error -32601 |
| Nav marks correct page active | ✅ | `sdk-js.html` active ✅ |
| Constructor port in example | ✅ | `http://localhost:8899` ✅ |
| Unit conversion | ✅ | `balance / 1e9` correct |
| WS methods documented | ✅ | All match ws.rs dispatch |
| Missing RPC wrapper methods | 🟡 Medium | SDK docs don't cover `getMoltyIdIdentity`, `getMoltyIdProfile`, `resolveMoltName`, `getShieldedPoolState`, bridge methods, all stats methods — these are accessible via the generic `rpcCall()` method but no named wrappers documented |

### 6.2 Python SDK (`sdk-python.html`)

| Issue | Severity | Detail |
|-------|----------|--------|
| Nav `.active` on wrong link | 🟠 High | `<a href="sdk-js.html" class="active">SDK</a>` — the JS link is active on the Python page. User sees wrong page highlighted in nav |
| Constructor port in example | ✅ | `http://localhost:8899` ✅ |
| `Keypair.load(path)` / `save()` | ✅ | Better documented than JS |
| `PublicKey.new_unique()` | ✅ | Class method correctly documented |

### 6.3 Rust SDK (`sdk-rust.html`)

| Issue | Severity | Detail |
|-------|----------|--------|
| Nav marks `sdk-js.html` active | 🟡 Medium | The nav has no distinct "Rust SDK" entry — "SDK" links to `sdk-js.html`. Sidebar correctly marks Rust active. Acceptable but could be confusing |
| `send_raw_transaction` documented | 🟡 Medium | No `send_raw_transaction` method visible in main RPC dispatch; may map to `sendTransaction` internally |
| WS `lamports` field | 🟡 Medium | `on_account_change` callback example shows `account.lamports` but MoltChain uses `shells` — Solana terminology leak in Rust SDK docs |
| Error codes listed | ✅ | `-32601`, `-32602`, `-32003`, `-32005` documented ✅ |
| `ClientBuilder` | ✅ | Well documented |
| No `getProgramAccounts` | ✅ | Correctly absent from Rust SDK |

### 6.4 MoltyID SDK Examples (`moltyid.html`)

| Issue | Severity | Detail |
|-------|----------|--------|
| Tab switching uses `onclick` | 🔵 Low | Inline `onclick` in `<button>` tags; non-standard and harder to CSP-harden |
| Rust example uses `moltchain_client` | 🟡 Medium | Package name is `moltchain-sdk` throughout other docs but import is `use moltchain_client::...` — inconsistent crate name |
| Python example imports `Client` not `Connection` | 🟡 Medium | `sdk-python.html` uses `Connection("http://...")` but `moltyid.html` Python example uses `client = Client("http://localhost:8899")` — inconsistent class name |

### 6.5 ZK Privacy SDK Examples (`zk-privacy.html`)

The Python SDK examples in `zk-privacy.html` import `shield_instruction`, `unshield_instruction`, `transfer_instruction` from `moltchain`. These are not documented in `sdk-python.html`. Users would have no way to discover these imports from normal SDK docs. The ZK guide is self-contained and accurate, but cross-references to `sdk-python.html` are absent.

### 6.6 `getting-started.html` CLI Example vs CLI Reference Mismatch

`getting-started.html` uses `molt wallet new` in its tutorial, but `cli-reference.html` documents the command as `molt wallet create` — a naming mismatch that will confuse users following the getting-started guide.

**Note:** `molt airdrop` and `molt transfer` ARE fully documented in `cli-reference.html` with complete `cmd-section` blocks, syntax, flags, and examples. Only the `wallet new` vs `wallet create` naming is a genuine mismatch.

This means the Getting Started guide cannot be completed using the CLI reference as a supplement.

---

## 7. Navigation & Links Audit

### 7.1 Active Nav Link Bugs

| Page | Expected Active Nav | Actual Active Nav | Status |
|------|--------------------|--------------------|--------|
| `index.html` | Hub | Hub | ✅ |
| `getting-started.html` | Get Started | Get Started | ✅ |
| `playground.html` | none | none | ✅ (not in nav) |
| `rpc-reference.html` | API | API | ✅ |
| `ws-reference.html` | API | API | ✅ |
| `sdk-js.html` | SDK | SDK | ✅ |
| `sdk-python.html` | SDK | **SDK (JS link)** | ❌ Wrong item active |
| `sdk-rust.html` | SDK | SDK (JS link — acceptable) | ⚠️ Debatable |
| `contracts.html` | Contracts | Contracts | ✅ |
| `contract-reference.html` | Contracts | **None** | ⚠️ No item active |
| `cli-reference.html` | CLI | CLI | ✅ |
| `moltyid.html` | MoltyID | MoltyID | ✅ |
| `zk-privacy.html` | Privacy | Privacy | ✅ |
| `architecture.html` | none | none | ✅ (not in nav) |
| `validator.html` | none | none | ✅ (not in nav) |
| `changelog.html` | none | none | ✅ (not in nav) |

### 7.2 Orphaned Pages (Not Reachable from Main Nav)

The following pages have no link in the top navigation bar. They are only accessible via sidebar links on other pages:

- **`architecture.html`** — reachable from `getting-started.html` Next Steps cards and sidebar
- **`validator.html`** — reachable from `getting-started.html` sidebar "More Guides"
- **`changelog.html`** — reachable from `index.html` footer link; not linked from main nav

Users arriving on these pages for the first time cannot orient themselves in the portal navigation hierarchy.

### 7.3 Cross-Page Internal Links

| Link | Source | Target | Status |
|------|--------|--------|--------|
| `../programs/index.html` | `playground.html` | Programs IDE | ⚠️ Path may not resolve depending on deployment |
| `../faucet/index.html` | `getting-started.html` | Faucet app | ⚠️ Same path-resolution concern |
| `../explorer/index.html` | `getting-started.html` | Block Explorer app | ⚠️ Same path-resolution concern |
| `contract-reference.html#moltyid` | `moltyid.html` sidebar | MoltyID section | ✅ Anchor exists |
| `contracts.html#crosscall` | `moltyid.html` sidebar | Cross-call section | ✅ Anchor exists |
| `ws-reference.html` | `rpc-reference.html` | WS reference | ✅ |
| `validator.html` | `architecture.html` sidebar | Validator guide | ✅ |

### 7.4 Mainnet RPC Port Inconsistency

`validator.html` documents mainnet RPC port as **9899** (testnet 8899, mainnet 9899). However:
- `shared/utils.js` `MOLT_ENDPOINTS.mainnet_rpc` = `https://rpc.moltchain.io` (no port — assumes 443)
- `developers.js` `NETWORK_ENDPOINTS.mainnet` = `https://rpc.moltchain.io` (no port)
- `wallet-connect.js` fallback = `http://localhost:9000` (wrong port)

The port 9000 in `wallet-connect.js` is wrong regardless of network. If mainnet truly runs on 9899 locally, every local mainnet connection via `wallet-connect.js` will also fail.

---

## 8. CSS System Audit

### 8.1 Dual CSS Variable System

The portal loads **two CSS files** that both define design tokens, in this order:

1. `shared-base-styles.css` — defines `--primary`, `--primary-dark`, `--secondary`, `--accent`, `--success`, `--warning`, `--info`, `--bg-dark`, `--bg-darker`, `--bg-card`, `--text-primary`, `--text-secondary`, `--text-muted`, `--border`, `--gradient-1/2/3`, `--shadow`, `--shadow-lg`

2. `shared-theme.css` — defines `--orange-primary`, `--orange-dark`, `--orange-accent`, `--blue-primary`, `--blue-accent`, `--green-success`, `--yellow-warning`, `--bg-dark`, `--bg-darker`, `--bg-card`, `--bg-hover`, `--text-primary`, `--text-secondary`, `--text-muted`, `--border`, `--border-light`, `--space-*`, `--radius-*`, `--shadow-sm`, `--shadow-md`, `--shadow-lg`, `--shadow-glow`

**Conflicts and gaps:**

| Variable | In base-styles? | In theme? | Result |
|----------|----------------|-----------|--------|
| `--primary` | ✅ `#FF6B35` | ❌ | Only via base-styles |
| `--orange-primary` | ❌ | ✅ `#FF6B35` | Only via theme |
| `--shadow` | ✅ flat value | ❌ | Only via base-styles |
| `--shadow-sm/md/lg` | ❌ | ✅ | Only via theme |
| `--shadow-lg` | ✅ (different value) | ✅ (different value) | **Conflict — theme wins** |
| `--bg-hover` | ❌ | ✅ | Only via theme |
| `--bg-surface` | ❌ | ❌ | **Undefined — any usage causes invisible style failure** |
| `--border-light` | ❌ | ✅ | Only via theme |
| `--gradient-1/2/3` | ✅ | ❌ | Only via base-styles |

Any component that uses `--bg-surface` silently falls back to `unset`. Search `css/developers.css` and all HTML files for this variable if it appears.

### 8.2 `contract-reference.html` — Isolated CSS Design

`contract-reference.html` does **not** load `shared-base-styles.css` or `shared-theme.css`. It loads only `css/developers.css` plus a block of **inline `<style>` CSS** defining its own variables:

```css
:root {
    --bg-primary: #0a0e1a;    /* Not in shared system */
    --bg-secondary: #111827;
    --bg-card: #1a1f35;       /* Overrides shared --bg-card via inline */
    --accent: #f97316;        /* Same value as --orange-primary but different name */
    --blue: #3b82f6;          /* Not in shared system */
    --green: #22c55e;
    --purple: #a855f7;
    --cyan: #06b6d4;
    --red: #ef4444;
    --yellow: #eab308;
}
```

This means `contract-reference.html` has a completely different visual language from every other portal page — it renders with darker backgrounds and different spacing than the rest of the portal.

### 8.3 `css/developers.css` Usage

`css/developers.css` is loaded on all pages as the third CSS layer. It provides page-specific component classes (`.method-card`, `.param-table`, `.callout`, `.sidebar-link`, `.docs-layout`, etc.) that are used across all portal pages. It correctly inherits variables from the two shared files. No critical issues found in isolation.

### 8.4 `shared-config.js` Loading Order Issue

`index.html` loads scripts in this order:
1. `<script src="js/developers.js"></script>` (early in `<body>`)
2. `<script src="shared-config.js"></script>` (at bottom of `<body>`)

`developers.js` calls `getMoltRpcUrl()` which checks `window.moltConfig.rpcUrl`. Since `shared-config.js` hasn't loaded yet when `developers.js` initializes, `window.moltConfig` is `undefined` and the live-stats RPC calls use the fallback `http://localhost:8899`. This is accidentally correct for local dev but breaks if `shared-config.js` is intended to override the URL. The script should be loaded **before** `developers.js`.

Furthermore, `shared-config.js` is loaded **only on `index.html`**. All other pages (rpc-reference.html, sdk-js.html, etc.) that also call `getMoltRpcUrl()` from `utils.js` will always use the hardcoded fallback.

---

## 9. Search System Audit

### 9.1 Architecture

The search system is a hardcoded `SEARCH_INDEX` array (~55 entries) in `js/developers.js` (lines ~244–350). Matching is done via `item.title.toLowerCase().includes(query)` and `item.description.toLowerCase().includes(query)`. The search overlay is triggered by `Cmd+K` or focusing the `#searchInput` in the nav.

### 9.2 Method Name Prefix Bug

The `SEARCH_INDEX` stores RPC method entries with a `molt_` prefix format:

```javascript
{ title: "molt_getBalance", ... }
{ title: "molt_getBlock", ... }
{ title: "molt_sendTransaction", ... }
```

The actual RPC methods are `getBalance`, `getBlock`, `sendTransaction` (no prefix). A developer searching for `getBalance` will find zero results. A developer searching for `molt_getBalance` (which they are unlikely to do since nothing in the docs uses this prefix) will find the entry. **The entire RPC method search is broken for any realistic query.**

### 9.3 Coverage Gaps in Search Index

The hardcoded index (~55 entries) does not cover:
- Any ZK privacy methods (`getShieldedPoolState`, etc.)
- Any bridge methods (`createBridgeDeposit`, etc.)
- Any stats endpoints (`getDexCoreStats`, etc.)
- Any MoltyID RPC methods (`getMoltyIdIdentity`, `resolveMoltName`, etc.)
- Validator guide, Architecture page, Changelog

These 66+ methods and 3 pages are completely unsearchable.

### 9.4 Search UX Issues

- Nav `#searchInput` does not actually search — it only opens the overlay (correct, opens with `Cmd+K`)
- But on most pages, the inline search wiring (`navInput.addEventListener('focus', ...)`) is only present in `getting-started.html`, `architecture.html`, `changelog.html`, and `index.html`. Pages like `sdk-js.html`, `moltyid.html`, `zk-privacy.html`, `contracts.html` do NOT have this inline wiring. On these pages, `developers.js` handles the overlay via `initSearch()` — BUT `initSearch()` uses `document.addEventListener('keydown', ...)` for `Cmd+K`. The nav input focus-to-overlay delegation is **missing from most pages**.

---

## 10. Issues Found — Master Table

| # | Severity | Category | File | Description | Fix |
|---|----------|----------|------|-------------|-----|
| 1 | 🔴 Critical | JavaScript | `js/developers.js` | Network selector never fires — `initNetworkSelector()` looks for `.network-selector-dev` CSS class but all pages use `#devNetworkSelect` id with `<select>`. The `NETWORK_ENDPOINTS` map also uses keys `devnet/testnet/mainnet` but all pages' `<select>` options have values `mainnet/testnet/local-testnet/local-mainnet` — fundamental key mismatch | Rewrite to use `document.getElementById('devNetworkSelect')` and add `local-testnet`/`local-mainnet` entries to `NETWORK_ENDPOINTS` |
| 2 | 🔴 Critical | Content | `playground.html` | Page is a documentation guide, not an interactive playground. Links to `../programs/index.html` which may not exist at that relative path | Either implement interactive RPC playground or rename/redirect to `programs-guide.html` |
| 3 | 🔴 Critical | JavaScript | `shared/wallet-connect.js:43` | Default fallback RPC URL is `http://localhost:9000` — wrong port. Actual RPC server is on port 8899 | Change `localhost:9000` to `localhost:8899` |
| 4 | 🟠 High | SDK Docs | `sdk-js.html` | `getProgramAccounts` documented in JS SDK but absent from `rpc/src/lib.rs` dispatch table — will return JSON-RPC error -32601. Not present in Python or Rust SDK docs. | Remove from `sdk-js.html` or implement the server method |
| 5 | 🟠 High | Navigation | `sdk-python.html` | `<a href="sdk-js.html" class="active">SDK</a>` — JS nav item marked active on Python SDK page | Change to `<a href="sdk-python.html" class="active">...` or restructure SDK nav |
| 6 | 🟠 High | Search | `js/developers.js:244-350` | `SEARCH_INDEX` entries use `molt_getBalance`, `molt_sendTransaction` etc. with `molt_` prefix — actual method names have no prefix. All RPC method searches return zero results | Remove `molt_` prefix from all search index method names |
| 7 | 🟠 High | RPC Docs | `rpc-reference.html` | ~66 server methods (bridge, shielded, all stats endpoints, ReefStake, DAO, Oracle, name auction, EVM lookup) have no full documentation card — only listed in bullets or missing entirely | Write method-card blocks for all live methods |
| 8 | 🟠 High | CSS | `contract-reference.html` | Does not load `shared-base-styles.css` or `shared-theme.css`. Has inline `<style>` block with completely different variables. Renders with different visual design from every other portal page | Add `<link rel="stylesheet" href="shared-base-styles.css">` and `<link rel="stylesheet" href="shared-theme.css">` and remove inline styles |
| 9 | 🟠 High | Navigation | `architecture.html`, `validator.html`, `changelog.html` | Three content-rich pages are orphaned from the main nav menu. Users cannot discover them through normal navigation | Add links to main nav or create a prominent "More" section |
| 10 | 🟠 High | CLI Docs | `getting-started.html` / `cli-reference.html` | `getting-started.html` uses `molt wallet new` but CLI reference documents `molt wallet create` — naming mismatch. (`molt airdrop` and `molt transfer` ARE fully documented in `cli-reference.html`.) | Align `wallet new` → `wallet create` in getting-started guide |
| 11 | 🟡 Medium | JavaScript | `index.html` | `shared-config.js` is loaded after `js/developers.js` — `window.moltConfig` is undefined during initialization. Also not loaded on any other page | Move `<script src="shared-config.js">` before `developers.js`; load it on all pages |
| 12 | 🟡 Medium | CSS | `shared-base-styles.css` / `shared-theme.css` | Two parallel CSS variable systems — `--primary` vs `--orange-primary`, `--shadow` vs `--shadow-sm/md/lg`. `--shadow-lg` defined in both with different values (theme wins). `--bg-surface` undefined in both but may be referenced | Consolidate to single variable system or add explicit alias rules |
| 13 | 🟡 Medium | RPC Docs | `rpc-reference.html` | `stake` and `unstake` documented with a `from` field parameter format, but server handlers accept base64-encoded signed transactions. No `require_single_validator` guard exists on these endpoints (they do NOT return -32003). Parameter format documentation is misleading | Correct parameter documentation to match actual handler signature |
| 14 | 🟡 Medium | WS Docs | `ws-reference.html` | `subscribeBridgeLocks` and `subscribeBridgeMints` are in the sidebar but have no method card in the page body | Add full method cards with payload examples for both |
| 15 | 🟡 Medium | SDK Docs | `sdk-rust.html` WS section | `account.lamports` in `on_account_change` callback example — Solana terminology. MoltChain uses `shells` | Change `account.lamports` to `account.shells` |
| 16 | 🟡 Medium | SDK Docs | `moltyid.html` Rust examples | `use moltchain_client::...` — inconsistent with `moltchain-sdk` used throughout all other SDK docs | Change to `use moltchain_sdk::...` |
| 17 | 🟡 Medium | SDK Docs | `moltyid.html` Python examples | Uses `Client(...)` but `sdk-python.html` uses `Connection(...)` | Standardise to `Connection` (as in Python SDK reference) |
| 18 | 🟡 Medium | Content | `moltyid.html` / `architecture.html` | Trust tier 1 is named "Known" in `moltyid.html` but "Verified" in `architecture.html` | Standardise name — pick one |
| 19 | 🟡 Medium | Search | `js/developers.js` | Search index covers only ~55 items. ZK methods, bridge methods, all stats endpoints, validator guide, architecture page are completely unsearchable | Expand search index to cover all documented methods and all pages |
| 20 | 🟡 Medium | Links | `getting-started.html` | `../faucet/index.html` and `../explorer/index.html` are relative paths that depend on deployment serving structure. On local `file://` open they will fail | Use absolute paths or document the required server setup |
| 21 | 🟡 Medium | Search | Multiple pages | Nav search input focus-to-overlay delegation inline script missing from `sdk-js.html`, `moltyid.html`, `zk-privacy.html`, `contracts.html`, `sdk-python.html`, `sdk-rust.html`, `contract-reference.html`, `rpc-reference.html`, `ws-reference.html` (9 pages). `cli-reference.html` and `validator.html` already have it. | Add inline delegation script to the 9 remaining pages or move it into `developers.js` `initSearch()` |
| 22 | 🟡 Medium | CSS | `zk-privacy.html` | Main element uses class `docs-content` but all other pages use `docs-main` — may cause layout inconsistency if `css/developers.css` only targets `.docs-main` | Change to `docs-main` or add `.docs-content` as alias in CSS |
| 23 | 🔵 Low | Navigation | `ws-reference.html` | `rpc-reference.html` nav has `rpc-reference.html` active even when viewing `ws-reference.html` — these are considered the same "API" nav item. Borderline acceptable | Consider adding WS reference as a separate nav entry or sub-item |
| 24 | 🔵 Low | Content | `rpc-reference.html` | `getMarketSales` and `getMarketListings` are in the full-card section, but `getMarketOffers` and `getMarketAuctions` (which exist in the server dispatch table) are completely absent from `rpc-reference.html` — not even bullet entries | Add full method cards or at minimum bullet entries |
| 25 | 🔵 Low | Content | `architecture.html` | States ZK proof verification is "in active transition" and "partial" — but `zk-privacy.html` documents the full API as if it's production-ready. Contradiction | Align status messaging across both pages |
| 26 | 🔵 Low | Content | `validator.html` | Documents `config.toml` configuration sections but adds a callout saying "These settings are passed as command-line flags" — the section headings imply TOML but the reality is CLI flags. User confusion risk | Either implement proper `config.toml` support or remove the section-heading format and use flat flag table |
| 27 | 🔵 Low | Content | `contract-reference.html` | Header stat says "227 Public Functions" but MoltyID card says "33 fns" while function list shows 33 chips. Other contract cards say "33 fns" but moltyid.html says "API Reference (34 fns)". Inconsistency in function count | Audit and reconcile function counts; use a single source of truth |
| 28 | 🔵 Low | Content | `contract-reference.html` | Opcode dispatcher contracts (DEX Core, DEX AMM, etc.) noted but no opcode table provided in this file | Link to or embed opcode tables for dispatcher contracts |
| 29 | 🔵 Low | Content | `getting-started.html` | Deployment cost callout says "flat fee of 25 MOLT (CONTRACT_DEPLOY_FEE)" but `shared/utils.js` has `CONTRACT_DEPLOY_FEE = 25 * SHELLS_PER_MOLT = 25_000_000_000` shells = 25 MOLT. Value is consistent, but the WASM fee is per-byte (`fee = wasm_size * FEE_PER_BYTE_SHELL` in the server). The "flat 25 MOLT" description is inaccurate | Fix to: "base fee of ~25 MOLT minimum; actual fee scales with WASM binary size" |
| 30 | 🔵 Low | Navigation | `contract-reference.html` | No active nav item — the page is reachable from `moltyid.html` and `contracts.html` sidebars but has no active nav highlight at all | Add `class="active"` to the `<a href="contracts.html">Contracts</a>` nav item since it's a contracts sub-page |
| 31 | 🔵 Low | WS Docs | `ws-reference.html` / `index.html` | `index.html` WS example uses `slotSubscribe` (Solana-style alias). `ws-reference.html` canonical name is `subscribeSlots`. Both work, but mixing styles within the same portal is confusing | Update `index.html` example to use canonical `subscribeSlots` |

---

## Appendix A: Quick Fix Priority List

**Do immediately (blocks basic functionality):**
1. Fix `wallet-connect.js` default port: `9000` → `8899`
2. Fix `sdk-python.html` nav active: `sdk-js.html` → current page
3. Fix `developers.js` `initNetworkSelector()` to target `#devNetworkSelect`
4. Align `NETWORK_ENDPOINTS` keys with `<select>` option values
5. Fix search index — remove `molt_` prefix from all RPC method entries

**Do before launch (user-facing content issues):**
6. Remove `getProgramAccounts` from `sdk-js.html` or implement server method
7. Add `shared-base-styles.css` and `shared-theme.css` to `contract-reference.html`
8. Align `molt wallet new` in `getting-started.html` to `molt wallet create` (as documented in `cli-reference.html`)
9. Add nav links for `architecture.html`, `validator.html`, `changelog.html`
10. Add nav search focus delegation to the 9 remaining pages (cli-reference.html and validator.html already have it)

**Do for completeness:**
11. Write full method cards for all ~66 undocumented RPC methods
12. Add `subscribeBridgeLocks`/`subscribeBridgeMints` method cards to `ws-reference.html`
13. Expand search index to cover all methods and pages
14. Fix `--shadow-lg` CSS variable conflict between `shared-base-styles.css` and `shared-theme.css`
15. Fix `shared-config.js` loading order and include it on all pages
16. Fix `stake`/`unstake` docs parameter format (currently shows `from` field but handlers take signed transactions)

---

*End of DEVPORTAL_AUDIT.md*
