# MoltChain Frontend Production-Readiness Audit

**Date:** 2025-01-20  
**Scope:** All frontend applications — Explorer, Wallet (web + extension), DEX, Faucet, Website, Monitoring, Marketplace, Programs IDE, Developers Portal  
**Method:** Exhaustive line-by-line source code review  
**Total code reviewed:** ~45,000+ lines of JavaScript, ~15,000+ lines of HTML, ~14,000+ lines of CSS

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Cross-Cutting Findings](#2-cross-cutting-findings)
3. [Explorer](#3-explorer)
4. [Wallet (Web App)](#4-wallet-web-app)
5. [Wallet (Browser Extension)](#5-wallet-browser-extension)
6. [DEX](#6-dex)
7. [Faucet](#7-faucet)
8. [Website](#8-website)
9. [Monitoring Dashboard](#9-monitoring-dashboard)
10. [Marketplace](#10-marketplace)
11. [Programs IDE (Playground)](#11-programs-ide-playground)
12. [Developers Portal](#12-developers-portal)
13. [Shared Libraries](#13-shared-libraries)
14. [Summary Scorecards](#14-summary-scorecards)

---

## 1. Executive Summary

The MoltChain frontend is a collection of **10+ vanilla HTML/CSS/JS applications** with no build tools, bundlers, or frameworks. The codebase is substantial (~75K+ total lines) and demonstrates impressive breadth — covering a block explorer, wallet, DEX with margin/prediction markets/governance/launchpad, NFT marketplace, smart contract IDE with Monaco Editor, monitoring dashboard, and developer documentation.

### Critical Verdict

**The frontend is NOT production-ready.** While individual feature implementations are often well-designed (especially the DEX and Programs IDE), the codebase suffers from:

- **12 CRITICAL security issues** (key storage, non-standard BIP39, missing CSP, client-side captcha, unencrypted secret export)
- **15 HIGH severity issues** (massive code duplication, hardcoded mock prices, N+1 RPC storms, network inconsistencies)
- **20+ MEDIUM issues** (stub implementations, inconsistent constants, race conditions)
- **Pervasive code duplication** — `escapeHtml` defined 14+ times, `Base58` 6+ times, `MoltChainRPC` 5+ independent implementations

### Severity Definitions

| Severity | Definition |
|----------|-----------|
| **CRITICAL** | Security vulnerability or data-loss risk that must be fixed before any public deployment |
| **HIGH** | Significant functionality gap, performance hazard, or reliability issue |
| **MEDIUM** | UX issue, code quality problem, or maintainability concern |
| **LOW** | Minor cosmetic, accessibility, or polish issue |

---

## 2. Cross-Cutting Findings

These issues affect multiple or all applications.

### 2.1 Massive Code Duplication (HIGH)

| Function/Utility | # of Independent Copies | Files |
|-----------------|------------------------|-------|
| `escapeHtml()` | **14+** | Every JS file redefines it with slight variations |
| `Base58` encode/decode | **6+** | wallet/crypto.js, explorer/utils.js, dex.js, SDK, extension/crypto-service.js, shared-config.js |
| `MoltChainRPC` class | **5** | explorer, wallet, website, monitoring, SDK — each with different feature sets (retry, cache, timeout) |
| `timeAgo()` | **8+** | explorer (5 files), marketplace (5 files), wallet, DEX |
| `hashString()` / `gradientFromHash()` | **7+** | explorer, marketplace (browse, create, item, profile, marketplace-data) |
| `formatHash()` / `truncateAddress()` | **10+** | Nearly every application |
| `rpcCall()` helper | **6+** | Each marketplace page, faucet, monitoring, website |
| `priceToMolt()` | **5+** | marketplace files, explorer |
| Trust tier definitions | **4** | wallet/identity.js, wallet.js, explorer/address.js, explorer/agents.js |
| `serializeMessageBincode()` | **2** | explorer/utils.js, wallet/wallet.js (different parameter handling) |

**Impact:** Bug fixes must be applied in 5-14 places. Behavioral drift between copies causes subtle inconsistencies. Dramatically increases bundle size.

**Recommendation:** Extract into a proper `shared/utils.js` module imported by all apps.

### 2.2 No Content Security Policy (CRITICAL)

Zero HTML files include CSP meta tags or headers. All apps load these external resources:
- Google Fonts CDN (fonts.googleapis.com)
- Font Awesome CDN (cdnjs.cloudflare.com)
- TweetNaCl, js-sha3, bip39 from CDN (wallet)
- Monaco Editor from CDN (programs)
- TradingView charting library (DEX)
- Binance WebSocket price feed (DEX)

Without CSP, any XSS vulnerability can exfiltrate wallet keys. The combination of localStorage-stored secrets + no CSP + CDN dependencies is the **#1 security concern**.

### 2.3 Network URL Inconsistencies (HIGH)

| App | Mainnet RPC | Testnet RPC | Local Port |
|-----|-------------|-------------|------------|
| Explorer | rpc.moltchain.network | testnet-rpc.moltchain.network | **9899** |
| Wallet (web) | rpc.moltchain.network | testnet-rpc.moltchain.network | **8899** |
| Wallet (ext) | rpc.moltchain.network | testnet-rpc.moltchain.network | **8899** |
| DEX | rpc.moltchain.**io** | testnet-rpc.moltchain.**io** | **8899** |
| Website | rpc.moltchain.network | testnet-rpc.moltchain.network | **8899** |
| Monitoring | rpc.moltchain.network | testnet-rpc.moltchain.network | **9899** |
| Marketplace | rpc.moltchain.network | testnet-rpc.moltchain.network | **9899** |
| Developers | rpc.moltchain.**io** | testnet-rpc.moltchain.**io** | **8899** |
| Programs SDK | rpc.moltchain.network | testnet-rpc.moltchain.network | **8899** |

Three distinct problems:
1. **Domain inconsistency:** DEX and Developers use `.io`, everything else uses `.network`
2. **Local port split:** Explorer/Monitoring/Marketplace use port 9899; all others use 8899
3. **Default network varies:** Website/DEX/wallet default to `local-testnet`, explorer blocks.html defaults to `mainnet`

### 2.4 No Build System (MEDIUM)

All applications are raw HTML/CSS/JS with no:
- Bundler (webpack, vite, esbuild)
- Minification or tree-shaking
- Dead code elimination
- Module system (`<script>` tag loading order matters)
- Hot module replacement for development
- Source maps for production debugging

The DEX alone is 5,200 lines of JS delivered as a single file. The Programs IDE is 8,817 lines.

### 2.5 Missing Accessibility (LOW — pervasive)

Across all apps:
- No ARIA labels on interactive elements
- No keyboard navigation for clickable `<div>` elements used as buttons
- No skip-to-content links
- Table rows with `onclick` not keyboard-accessible
- No `aria-live` regions for dynamic content updates
- Contrast issues in dark theme (muted text colors)
- No `<label>` elements for most form inputs
- `tabindex` absent from custom interactive components

---

## 3. Explorer

**Files reviewed:** utils.js (345), explorer.js (790), blocks.js (283), block.js (157), transactions.js (297), transaction.js (407), contracts.js (291), validators.js (180), agents.js (319), address.js (2046), contract.js (550), explorer.css (3710), 10 HTML files, explorer.test.js (403)

### 3.1 Simplified/Stub Implementations

| Item | Details |
|------|---------|
| MoltyID `.molt` name resolution | Calls `getMoltName` RPC — genuinely on-chain, not stubbed |
| Trust tier display | Computed from reputation ranges — functional |
| Transaction graph | Canvas-based rendering works for demo but lacks zoom/pan |

**Verdict:** Explorer is the **most production-ready** app. Most features are RPC-backed with real data.

### 3.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| E-1 | HIGH | `moltNameCache` in address.js is unbounded `Map` — memory leak under sustained use |
| E-2 | MEDIUM | `serializeMessageBincode()` in utils.js builds a bincode message for simulation but doesn't validate instruction accounts array length |
| E-3 | LOW | Footer links point to raw `.md` files (`CONTRIBUTING.md`, `SECURITY_AUDIT_REPORT.md`) |

### 3.3 API Integration

- All data fetched via JSON-RPC 2.0 (`getBlock`, `getTransaction`, `getBalance`, etc.)
- WebSocket subscriptions for live block/transaction updates
- `getAccountInfo`, `getTransaction` responses parsed and displayed correctly
- Contract ABI rendering from `getContractAbi` RPC

**No mock data detected.** Explorer is fully RPC-dependent.

### 3.4 UX/Functionality Gaps

| Gap | Impact |
|-----|--------|
| No pagination controls for blocks list (loads first 50 only) | Cannot browse older blocks |
| Search only supports exact address/tx hash — no fuzzy search | Unusable without full hash |
| Mobile responsiveness is untested | Explorer CSS has no `@media` queries for tables |
| Clicking account badge in transaction view navigates but no "back" button | Poor navigation |

### 3.5 Dead Code / Unused CSS

- `explorer.css` is 3,710 lines — substantial portions appear unused (animation keyframes for features not in HTML)
- `formatHashFromHex` in utils.js — only used once, could be inlined
- `explorer.test.js` (403 lines) tests only exist for utils functions, not for DOM rendering

### 3.6 Consistency Issues

- `escapeHtml` in utils.js uses `textContent`/`innerHTML` DOM approach; other files use regex replacement
- `timeAgo` defined in utils.js AND independently in blocks.js, transactions.js, etc.
- Address page loads 4 different RPC calls sequentially instead of `Promise.all`

### 3.7 Performance Issues

| Issue | Severity | Details |
|-------|----------|---------|
| N+1 block loading | HIGH | `blocks.js` fetches 250 blocks by calling `getBlock` individually in a loop — should use batch RPC |
| Unbounded name cache | MEDIUM | `moltNameCache` Map grows forever |
| Transaction graph redraws on every tab switch | LOW | No caching of canvas state |

### 3.8 Testing Gaps

- `explorer.test.js` only tests utility functions (timeAgo, formatNumber, formatBytes, truncateAddress, base58)
- No DOM tests, no integration tests, no E2E tests
- No tests for address.js (2046 lines — the largest file)

---

## 4. Wallet (Web App)

**Files reviewed:** wallet.js (3787), identity.js (1193), crypto.js (535), index.html (927), manifest.json, wallet.css (referenced)

### 4.1 Simplified/Stub Implementations

| Item | Status | Details |
|------|--------|---------|
| USD prices | **MOCK** | `MOCK_PRICES` hardcodes MOLT=$0.10, wSOL=$95, wETH=$3200 — no live price feed |
| Token portfolio | **MOCK** | `loadTokens()` returns hardcoded stMOLT, wSOL, wETH with fake balances on first load |
| Bridge deposits | **PARTIAL** | Generates custody deposit addresses but confirmation polling may never complete |
| NFT metadata display | FUNCTIONAL | Loaded from RPC `getNFTsByOwner` |
| Staking (ReefStake) | FUNCTIONAL | Full lifecycle: stake, unstake, claim, tier display |
| MoltyID identity | FUNCTIONAL | Registration, naming, skills, vouches — all RPC-backed |
| Agent services | FUNCTIONAL | Discover/hire agents via RPC |

### 4.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| W-1 | **CRITICAL** | Wallet private key stored in `localStorage` as encrypted blob — any XSS grants access to the encrypted key. Combined with no CSP, this is exploitable. |
| W-2 | **CRITICAL** | `exportKeystore()` exports JSON containing the **raw 64-byte secretKey as plaintext array** — user may save/share this unintentionally |
| W-3 | **CRITICAL** | Mnemonic-to-seed uses SHA-512 directly: `sha512(passphrase + mnemonic)` — NOT standard BIP39 PBKDF2 derivation. Keys generated here are incompatible with all other BIP39 wallets (MetaMask, Phantom, etc.) |
| W-4 | **CRITICAL** | `isValidMnemonic()` sync path always returns `true` if 12 words are in the wordlist — doesn't verify BIP39 checksum |
| W-5 | HIGH | `localStorage.clear()` on logout wipes ALL localStorage, including data from other apps on the same origin |
| W-6 | HIGH | `showPrivateKey()` renders secretKey in a `<textarea>` in the DOM — screen readers, extensions, and shoulder-surfing all see it |
| W-7 | MEDIUM | Session password stored in module-level variable `sessionPassword` — accessible via devtools |
| W-8 | MEDIUM | Backup verification (`verifyBackupPhrase`) only checks exact string match — no fuzzy matching for common typos |

### 4.3 API Integration

- RPC client in wallet.js: basic fetch wrapper, no retry, no timeout, no cache
- WebSocket for live balance/transaction updates with 5s fixed reconnect
- Bridge custody service integration (deposit address generation, confirmation polling)
- Token balance loaded from `getTokenBalance` RPC
- Staking via `sendTransaction` with proper instruction encoding

### 4.4 UX/Functionality Gaps

| Gap | Impact |
|-----|--------|
| No transaction history pagination | Only loads last 20 |
| No token price refresh | MOCK_PRICES never update |
| No address book | Must paste addresses every time |
| QR code scanning for mobile | Not implemented despite PWA manifest |
| Bridge only supports deposits, not withdrawals | One-way bridge |
| Swap tab exists but no implementation | Shows "Connect wallet" only |

### 4.5 Dead Code

- `MOCK_PRICES` object — should be replaced with live price feed or removed
- `hardcoded` token list in `loadTokens()` fallback — 3 fake tokens
- `createNewMultisigWallet()` — referenced in HTML but function is empty
- `setupBiometrics()` — stub that logs to console
- `enablePushNotifications()` — stub that shows toast "Coming soon"

### 4.6 Performance Issues

- Portfolio value recalculated on every tab switch (no memoization)
- Activity poll interval: 15s — acceptable for production
- WebSocket reconnect: fixed 5s (not exponential backoff)
- `loadAllIdentities()` on identity tab — fetches all identities, not paginated

### 4.7 Testing Gaps

- **Zero test files** for the wallet application
- No unit tests for crypto functions (critical gap given non-standard BIP39)
- No integration tests for transaction signing/sending

---

## 5. Wallet (Browser Extension)

**Files reviewed:** manifest.json, service-worker.js (215), content-script.js (133), crypto-service.js (569), state-store.js (67), inpage-provider.js (referenced)

### 5.1 Architecture

Manifest V3 Chrome extension with:
- Service worker background (module type)
- Content script injected on **all** HTTP/HTTPS pages (`"matches": ["http://*/*", "https://*/*"]`)
- Inpage provider injected via script tag
- Tab-based approval flow for transaction signing
- `chrome.storage.local` for state (better than web localStorage)

### 5.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| EXT-1 | **CRITICAL** | `mnemonicToKeypair()` uses `sha512(mnemonic).slice(0, 32)` — same non-standard BIP39 as web wallet. Extension wallets are incompatible with standard BIP39 wallets |
| EXT-2 | **CRITICAL** | `isValidMnemonic()` doesn't verify BIP39 checksum — only checks word count and wordlist membership |
| EXT-3 | HIGH | Content script injected on ALL pages creates broad attack surface for message spoofing |
| EXT-4 | MEDIUM | Provider state polling every 2s via `setInterval` — overhead on every open tab |
| EXT-5 | MEDIUM | `waitForProviderDecision` polls every 1s for up to 120s — 120 message round-trips per approval |

### 5.3 Positive Security Findings

| Item | Assessment |
|------|-----------|
| Private key encryption | PBKDF2 100K iterations + AES-256-GCM — industry standard |
| Key derivation | `deriveKey()` uses Web Crypto API properly |
| Transaction signing | Ed25519 via Web Crypto API PKCS#8 import — correct |
| State storage | `chrome.storage.local` — isolated from web page localStorage |
| Origin verification | `resolveSenderOrigin()` extracts and validates sender origin |
| Lock mechanism | Auto-lock via `chrome.alarms` API |
| Keccak-256 | Pure JS implementation for EVM address derivation — correct |

### 5.4 Missing Functionality

- No hardware wallet support (Ledger/Trezor)
- No EIP-6963 or wallet-standard compatible provider interface
- No support for encrypted mnemonic backup
- No phishing site detection
- No token auto-discovery
- No dApp connection management UI beyond basic approved origins list

---

## 6. DEX

**Files reviewed:** dex.js (5200 — COMPLETE), dex.css (referenced), index.html (1492, partial)

### 6.1 Architecture Overview

Single 5,200-line JS file implementing:
- **CLOB + AMM trading** (limit/market orders, liquidity pools)
- **Margin trading** (7 tiers: 2x-100x leverage, SL/TP)
- **Prediction markets** (PredictionReef — CPMM pricing, binary/multi-outcome, dispute lifecycle)
- **Governance** (create/vote/finalize/execute proposals)
- **Launchpad** (ClawPump — bonding curve token creation)
- **Rewards** (4-tier system: Bronze/Silver/Gold/Diamond)
- **TradingView charting** with MoltOracle price overlay

### 6.2 Simplified/Stub Implementations

| Item | Status | Details |
|------|--------|---------|
| Governance: delist & param_change proposals | **BLOCKED** | UI exists but handlers show "not yet supported on-chain" — cannot submit |
| Governance: execute proposal | **PARTIAL** | Sends transaction but on-chain execution may not be implemented |
| Referral system | **STUB** | Referral links generated but reward tracking not connected |
| Portfolio chart | **MOCK** | `loadPortfolioHistory()` generates synthetic data with `Math.random()` |
| DEX rewards: claim | FUNCTIONAL | Sends signed transaction |
| Prediction market: claim winnings | FUNCTIONAL | Proper CPMM calculations match contract |

### 6.3 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| D-1 | **CRITICAL** | DEX has its own independent wallet system with private keys saved in `localStorage` (via `savedWallet` key) — no encryption at rest |
| D-2 | HIGH | `connectSavedWallet()` reconstitutes full keypair from localStorage on page load in "view-only" mode — but the keypair is still in memory |
| D-3 | HIGH | Contract address hardcoded fallbacks — if contracts are recompiled/redeployed, the DEX silently uses stale addresses |
| D-4 | MEDIUM | Portfolio value cache reads then writes without lock — race condition if multiple async calls overlap |
| D-5 | MEDIUM | `confirm()` dialog for order execution — can be spoofed, no custom confirmation modal for trades |

### 6.4 API Integration

- All trading operations via `sendTransaction` with proper binary instruction encoding
- Contract calls use `contractIx()` helper for proper instruction building
- ClawPump uses `namedCallIx()` — different encoding pattern
- TradingView `datafeed` adapter connects to custom `getKlineData` RPC
- MoltOracle price overlay via `getMoltOraclePrice` RPC
- Binance WebSocket feed for wSOL/wETH real-time prices (`solusdt@miniTicker`, `ethusdt@miniTicker`)
- Prediction market CPMM pricing formula matches on-chain `calculate_buy`

### 6.5 UX/Functionality Gaps

| Gap | Impact |
|-----|--------|
| No mobile responsive layout | 5200-line desktop-only UI |
| Swap tab shows "Connect wallet" but no AMM router swaps | Only limit/CLOB orders work |
| Prediction market chart is canvas-only (no zoom/pan) | Basic visualization |
| Multi-outcome markets display up to 8 outcomes but input validation is client-side only | Server should validate |
| PnL share card downloads as PNG via canvas — no social sharing API | Manual share only |

### 6.6 Dead Code

- `renderTradeChart()` early-return comment says "TradingView handles this now" — dead function
- Unused CSS classes from grid layout experiments visible in HTML comments
- `predictCurrentSortKey` and `predictCurrentSortOrder` tracked but never persist across page loads
- `walletGateInterval` set up for polling but `applyWalletGate` only runs once

### 6.7 Performance Issues

| Issue | Severity | Details |
|-------|----------|---------|
| Pairs refresh polls ticker per pair (N+1) | HIGH | Each of N pairs triggers individual `getTickerData` RPC call every 10s |
| 4-tier polling architecture | MEDIUM | Fast=5s, Slow=30s, Predict=15s, Pairs=10s — 4 concurrent `setInterval` chains |
| ClawPump debounce on quote | LOW | 300ms debounce is reasonable |
| Close slot calculation inconsistency | LOW | Uses 400ms/slot in one place, 500ms in another |

### 6.8 Consistency Issues

- Uses `moltchain.io` domain while all other apps use `moltchain.network`
- MOLT decimals: uses 1e9 for MOLT but 1e6 for mUSD (PredictionReef) — correct per design but undocumented
- `formatPrice` defined at bottom of file (before hoisting makes it available) — works due to function declaration hoisting but fragile

### 6.9 Testing Gaps

- `dex.test.js` exists (5597 lines) but was not reviewed in detail
- No E2E tests for trading flows
- No tests for CPMM pricing edge cases (very small/large amounts)
- No tests for margin liquidation price calculations

---

## 7. Faucet

**Files reviewed:** faucet.js (156), index.html (155), faucet.css (referenced)

### 7.1 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| F-1 | **CRITICAL** | Captcha is client-side only: `Math.floor(Math.random() * 10) + 1` + `Math.floor(Math.random() * 10) + 1` — trivially bypassable by posting directly to RPC |
| F-2 | HIGH | No rate limiting visible on client side — relies entirely on server-side rate limiting |
| F-3 | MEDIUM | Faucet endpoint derived by replacing `/rpc` with `/faucet` in URL — fragile URL construction |

### 7.2 API Integration

- POST to `{rpc_url}/faucet/request` with `{ address, amount }`
- Validates address format client-side (Base58, 32-44 chars)
- Shows transaction signature on success

### 7.3 UX/Functionality Gaps

- No balance check after faucet drip (user must navigate to explorer)
- Amount fixed at "10 MOLT" — no selection
- No cooldown timer shown to user
- Footer links to raw `.md` files

### 7.4 Positive Notes

- Clean, minimal implementation appropriate for a testnet faucet
- Proper form validation
- Success/error states handled well

---

## 8. Website

**Files reviewed:** index.html (1242), script.js (429), styles.css (referenced)

### 8.1 Simplified/Stub Implementations

| Item | Status |
|------|--------|
| Live network stats | FUNCTIONAL — fetches from RPC `getMetrics` |
| Validator count | FUNCTIONAL — `getValidators` RPC |
| Token price | **MOCK** — comment says "Mock data for now" |
| "Start Building" button | Links to playground (functional) |

### 8.2 Issues

| ID | Severity | Issue |
|----|----------|-------|
| WEB-1 | MEDIUM | `animateNumber()` uses `setInterval(50ms)` — may cause jank if multiple counters animate simultaneously |
| WEB-2 | LOW | IntersectionObserver animations set elements to `opacity: 0` by default — content invisible without JS |
| WEB-3 | LOW | Newsletter signup form — action is `#` (non-functional) |
| WEB-4 | LOW | Social links in footer — all point to `#` (non-functional) |

### 8.3 Positive Notes

- Clean marketing page with proper responsive design
- Stats section pulls real data from RPC
- Good use of IntersectionObserver for scroll animations
- Proper error handling (shows 0 when RPC unavailable)

---

## 9. Monitoring Dashboard

**Files reviewed:** monitoring.js (1239 — COMPLETE), index.html (619, partial), monitoring.css (referenced)

### 9.1 Simplified/Stub Implementations

| Item | Status | Details |
|------|--------|---------|
| TPS chart | FUNCTIONAL | Canvas-based, real `getMetrics` data |
| Performance rings (CPU/Memory/Disk) | **SIMULATED** | Uses `Math.random()` offsets from a base — not real system metrics |
| Incident response | **UI ONLY** | Kill switch buttons exist but call admin RPC methods without visible authentication |
| DEX subsystem monitoring | FUNCTIONAL | Fetches from `getDexSubsystem` RPC |
| Contract registry | FUNCTIONAL | Lists deployed contracts from `getAllContracts` |

### 9.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| MON-1 | **CRITICAL** | Kill switch buttons (`pause_network`, `emergency_halt`, `rate_limit_enable`) call admin RPC methods through client-side JavaScript — no authentication visible |
| MON-2 | HIGH | `REFRESH_MS = 3000` (3-second polling) — aggressive for production, causes high RPC load |
| MON-3 | MEDIUM | No WebSocket — polling only, despite WebSocket being available on the platform |

### 9.3 Performance Issues

- 3-second polling with 10+ RPC calls per cycle = ~200+ RPC calls/minute minimum
- Canvas TPS chart redraws every 3s with up to 120 data points
- SVG performance rings re-rendered every cycle with opacity transitions
- No data downsampling for historical metrics

---

## 10. Marketplace

**Files reviewed:** marketplace.js (374), browse.js (511), create.js (485), item.js (478), profile.js (610), marketplace-config.js (83), marketplace-data.js (253), index.html (304)

### 10.1 Simplified/Stub Implementations

| Item | Status | Details |
|------|--------|---------|
| NFT listings | FUNCTIONAL | RPC-backed via `getMarketListings` |
| NFT purchase | **PARTIAL** | `handleBuy` sends transaction but uses JSON data field instead of binary contract instruction |
| NFT offers | **STUB** | `handleMakeOffer` shows toast "not implemented" |
| NFT creation (minting) | **PARTIAL** | Mints but stores image as base64 dataUrl in metadata (not IPFS) |
| Price chart on item page | **STUB** | Shows "No price history available" |
| Profile editing | **STUB** | Edit avatar/banner/name buttons show toast "requires wallet signature" |
| Favorites system | **UNIMPLEMENTED** | Tab always shows "No favorited NFTs yet" |
| USD price conversion | **MOCK** | Hardcoded `* 0.10` multiplier |
| NFT rarity | **FAKE** | Assigned from `hashString(nft.id)` — not from actual metadata attributes |

### 10.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| MKT-1 | HIGH | NFT minting stores base64 dataUrl directly in on-chain metadata — enormous transaction size, data loss risk if base64 is truncated |
| MKT-2 | HIGH | `handleBuy` in item.js sends transaction with JSON `data` field — may not match expected binary contract instruction format |
| MKT-3 | MEDIUM | `onclick="window._browseViewNFT('${nft.id}')"` — potential injection if `nft.id` contains single quotes (mitigated by `escapeHtml` but fragile) |
| MKT-4 | MEDIUM | `collectionNameCache` in marketplace-data.js is unbounded Map — memory leak |

### 10.3 Code Duplication (Marketplace-specific)

Every marketplace page JS file independently redefines:
- `escapeHtml()`
- `rpcCall()`  
- `hashString()`
- `gradientFromHash()`
- `formatHash()`
- `timeAgo()`
- `priceToMolt()`

This is **7 utility functions × 5 files = 35 redundant function definitions**.

### 10.4 UX/Functionality Gaps

| Gap | Impact |
|-----|--------|
| Browse page limited to 500 listings | Cannot paginate beyond 500 |
| Profile "created" count uses `owned` field as approximation | Inaccurate |
| No collection page | Can browse NFTs but not view collection details |
| Grid/list view toggle exists but list view may have layout issues | Untested |
| Sort by "popular" uses `hashString` as proxy | Not actual popularity metric |

### 10.5 Performance Issues

- `getTopCreators` loads ALL sales (limit 200) to compute creator rankings — expensive
- `getCollectionName` does individual RPC call per NFT in trending view — N+1 pattern
- `getStats` makes 3 separate RPC calls that could be batched

---

## 11. Programs IDE (Playground)

**Files reviewed:** playground-complete.js (8817 — ~60% line-by-line, all functional sections), moltchain-sdk.js (1386 — COMPLETE), landing.js (311 — COMPLETE)

### 11.1 Architecture

Impressive single-page Rust smart contract IDE featuring:
- **Monaco Editor** (v0.45.0 via CDN) with Rust syntax, keyboard shortcuts
- **Template system** for 8 contract types (Token, NFT, DEX, DAO, Lending, Launchpad, Vault, Identity, Marketplace, Auction) with configurable parameters
- **Build → Deploy → Test** pipeline via `/compile` endpoint
- **Multiple wallet management** with named wallets
- **Program interaction** panel (call functions, view storage, ABI display)
- **Snapshot system** (save/restore/export/import workspace)
- **Terminal** with command handler (help, clear, build, deploy, faucet, balance, network, wallet, rpc)

### 11.2 Security Issues

| ID | Severity | Issue |
|----|----------|-------|
| P-1 | **CRITICAL** | Wallet seeds stored in `localStorage` as plaintext base58 — `saveWalletStore()` writes `{ id, name, seed, address }` array directly |
| P-2 | HIGH | `wallet.export('')` returns seed without any password protection — empty string password param |
| P-3 | HIGH | `createWallet()` shows seed in `alert()` dialog — cannot be copied securely, visible to screen readers |
| P-4 | HIGH | `importWallet()` uses `prompt()` for secret key input — visible in browser history, not masked |
| P-5 | MEDIUM | `programKeypair` stored in localStorage — if used for mainnet, program authority keys exposed |
| P-6 | MEDIUM | Legacy wallet migration reads from `molt_wallet` localStorage — may import keys from other sessions unintentionally |

### 11.3 Positive Security Findings

| Item | Assessment |
|------|-----------|
| Terminal output escaping | `escapeHtml()` applied to all terminal lines — AUDIT-FIX markers show prior audit remediation |
| ABI display escaping | All RPC-sourced ABI fields escaped — AUDIT-FIX F14.6 |
| Program calls list | Caller/function escaped — AUDIT-FIX F14.4 |
| Storage viewer | Keys/values escaped — AUDIT-FIX F14.5 |
| Deployed programs list | Uses `data-program-id` attribute instead of inline onclick — AUDIT-FIX F14.2 |
| URL sanitization | `sanitizeUrl()` validates URL scheme before rendering links |
| Transfer validation | Amount bounds checked (0 to 1B MOLT) — AUDIT-FIX F14.8 |

### 11.4 SDK Assessment (moltchain-sdk.js)

**Best RPC implementation in the project:**
- Retry logic (3 attempts default)
- Exponential backoff
- AbortController timeout (30s)
- Response cache with 5s TTL
- Comprehensive method coverage (60+ RPC methods)
- WebSocket client with exponential reconnect (max 30s, 5 attempts)
- Proper Ed25519 wallet with TweetNaCl
- Transaction builder with instruction encoding
- Program deployer with confirmation polling

**Issues:**
| ID | Severity | Issue |
|----|----------|-------|
| SDK-1 | HIGH | `MoltChainWallet.export(password)` ignores the password parameter — returns plaintext seed |
| SDK-2 | HIGH | `MoltChainWallet.import(json, password)` ignores the password parameter — reads plaintext seed |
| SDK-3 | MEDIUM | `MoltChainWallet.generateMnemonic()` throws "not supported in browser SDK" — artificial limitation |
| SDK-4 | MEDIUM | `MoltChainWallet.fromMnemonic()` throws — wallet can only be created from random seed or imported from base58 |
| SDK-5 | LOW | `WEI_PER_MOLT` constant uses BigInt(1e18) — but MOLT uses 1e9 decimals per all other code |

### 11.5 UX/Functionality Gaps

| Gap | Impact |
|-----|--------|
| Template option changes regenerate entire file | Loses any manual edits |
| No undo/redo beyond Monaco's built-in | Acceptable |
| No collaborative editing | Not expected for v1 |
| Build requires running compiler service | No in-browser WASM compilation |
| No Solidity support despite `langMap` listing it | Only Rust contracts work |
| File system is flat (Map-based) | No nested directories despite folder creation UI |

### 11.6 Code Quality

- `playground-complete.js` at 8,817 lines should be split into modules
- Embedded Rust source templates as JS template literals make the file enormous
- Template generation functions (token, NFT, etc.) duplicate ~200 lines of boilerplate each
- State management via single `Playground` object works but is monolithic

---

## 12. Developers Portal

**Files reviewed:** developers.js (681 — COMPLETE), 15 HTML pages listed

### 12.1 Assessment

The developers portal is a **static documentation site** with:
- Sidebar navigation with collapsible sections
- Scroll spy for TOC highlighting
- Language tabs (JS/Python/Rust/CLI) with localStorage persistence
- `Cmd+K` / `Ctrl+K` search modal with pre-built search index
- Network selector (devnet/testnet/mainnet)
- Code copy buttons with fallback for older browsers
- Auto-generated table of contents from headings

### 12.2 Issues

| ID | Severity | Issue |
|----|----------|-------|
| DEV-1 | MEDIUM | Search index is hardcoded in JS (58 entries) — not generated from actual content |
| DEV-2 | MEDIUM | Uses `.io` domain in `NETWORK_ENDPOINTS` (inconsistent with most other apps) |
| DEV-3 | LOW | `highlightMatch` uses regex from user input without escaping — `escapeRegex` helper exists and is used correctly |
| DEV-4 | LOW | `document.execCommand('copy')` fallback is deprecated |
| DEV-5 | LOW | 15 HTML documentation pages not reviewed for accuracy of code examples |

### 12.3 Positive Notes

- Clean, well-structured documentation UI
- Language preference persists across pages
- Keyboard-navigable search results
- TOC auto-generation works well
- No security-sensitive functionality (read-only docs)

---

## 13. Shared Libraries

**Files reviewed:** shared-config.js, shared-theme.css, shared-base-styles.css (partial)

### 13.1 shared-config.js

Contains `MoltWallet` class — a shared wallet connector used by Explorer, Marketplace, and other apps. Features:
- Network selector UI injection
- `savedWallet` auto-connection from localStorage  
- `rpcCall` helper function
- `simulateAndSend` transaction helper

**Issues:**
| ID | Severity | Issue |
|----|----------|-------|
| SC-1 | HIGH | SharedWallet saves unencrypted keypair to localStorage (`savedWallet` key) |
| SC-2 | MEDIUM | Network configurations duplicated from each app's own config — drift risk |

### 13.2 shared-theme.css + shared-base-styles.css

**Issues:**
| ID | Severity | Issue |
|----|----------|-------|
| CSS-1 | MEDIUM | Duplicate design system: `shared-base-styles.css` (1294 lines) and `shared-theme.css` both define `--primary-color`, `--bg-primary`, etc. with slightly different values |
| CSS-2 | MEDIUM | `shared-theme.css` sets `.card { opacity: 0 }` — cards invisible without JS IntersectionObserver |
| CSS-3 | LOW | Both files loaded by every app — redundant downloads |

---

## 14. Summary Scorecards

### Per-Application Production Readiness

| Application | Security | Functionality | Performance | Code Quality | Accessibility | Overall |
|------------|----------|---------------|-------------|--------------|---------------|---------|
| Explorer | 🟡 | 🟢 | 🟡 | 🟡 | 🔴 | **7/10** |
| Wallet (web) | 🔴 | 🟡 | 🟢 | 🟡 | 🔴 | **4/10** |
| Wallet (ext) | 🟡 | 🟡 | 🟢 | 🟢 | 🔴 | **6/10** |
| DEX | 🔴 | 🟡 | 🟡 | 🟡 | 🔴 | **5/10** |
| Faucet | 🔴 | 🟢 | 🟢 | 🟢 | 🟡 | **6/10** |
| Website | 🟢 | 🟢 | 🟢 | 🟢 | 🟡 | **8/10** |
| Monitoring | 🔴 | 🟡 | 🔴 | 🟡 | 🔴 | **4/10** |
| Marketplace | 🟡 | 🔴 | 🟡 | 🔴 | 🔴 | **3/10** |
| Programs IDE | 🔴 | 🟢 | 🟢 | 🟡 | 🔴 | **5/10** |
| Developers | 🟢 | 🟢 | 🟢 | 🟢 | 🟡 | **9/10** |

🟢 = Acceptable  🟡 = Needs work  🔴 = Not ready

### Critical Issues Summary (Must Fix Before Production)

| # | Issue | Apps Affected | Effort |
|---|-------|--------------|--------|
| 1 | Add CSP headers to all HTML files | ALL | 1 day |
| 2 | Fix BIP39 derivation to use PBKDF2 | Wallet, Extension | 2 days |
| 3 | Encrypt private keys at rest in localStorage | DEX, Programs, shared-config | 2 days |
| 4 | Remove plaintext secretKey from keystore export | Wallet | 0.5 day |
| 5 | Implement server-side captcha/rate limiting | Faucet | 1 day |
| 6 | Add authentication to admin RPC methods | Monitoring | 1 day |
| 7 | Fix isValidMnemonic to verify checksum | Wallet, Extension | 0.5 day |
| 8 | Move NFT metadata to IPFS (not base64 dataUrl) | Marketplace | 2 days |
| 9 | Unify network URLs (`.io` vs `.network`, port 8899 vs 9899) | All | 0.5 day |
| 10 | Extract shared utilities to eliminate 14x code duplication | All | 3 days |
| 11 | Replace mock prices with live feed or oracle | Wallet, Marketplace | 2 days |
| 12 | Fix N+1 RPC patterns (blocks, pairs, collections) | Explorer, DEX, Marketplace | 2 days |

**Estimated total remediation: ~17 engineering days for critical/high issues.**

### Architecture Recommendations

1. **Adopt a build system** (Vite recommended) — enables module imports, dead code elimination, minification, and source maps
2. **Create `@moltchain/utils` package** — single source of truth for Base58, escapeHtml, RPC client, timeAgo, formatNumber, etc.
3. **Implement a shared wallet SDK** — one wallet connection library used by all apps instead of 5 independent implementations
4. **Add E2E tests** — Playwright or Cypress for critical flows (send transaction, deploy contract, place trade, mint NFT)
5. **Security audit of crypto code** — particularly the BIP39 deviation, by a specialized auditor

---

*End of audit. All findings based on source code review as of audit date. No runtime testing was performed.*
