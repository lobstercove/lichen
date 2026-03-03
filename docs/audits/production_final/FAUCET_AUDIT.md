# MoltFaucet Production Audit Report
**Date:** February 27, 2026
**Auditor:** Senior Blockchain Developer (Deep Review)
**Re-verified:** February 27, 2026 — code read against every finding
**Scope:** `/Users/johnrobin/.openclaw/workspace/moltchain/faucet/`
**Chain:** MoltChain (custom Rust blockchain, 400 ms finality, WASM + EVM contracts)

---

## ⚠️ Re-verification Status (Feb 27, 2026)

All source files (`src/main.rs`, `faucet.js`, `index.html`, `shared-config.js`, `shared/utils.js`, `docker-compose.yml`) were re-read line by line against every issue.

**Result: 0 of 21 original issues have been fixed.**
Additionally, 3 new issues were found (issues #22–24).

---

## 1. Executive Summary

The MoltFaucet is a **testnet token faucet** consisting of:
- A static HTML/JS frontend served from `faucet/index.html`
- A Rust/Axum HTTP backend (`faucet/src/main.rs`, port configurable via `PORT` env)
- A shared JS config (`faucet/shared-config.js`) providing `MOLT_CONFIG`
- A shared utility library (`faucet/shared/utils.js`) providing `escapeHtml`, `moltRpcCall`, base58 helpers, and the auto-wired chain status bar

The overall architecture is **sound**: the backend is correctly responsible for rate limiting, address validation, and issuing signed native-MOLT transfer transactions via `sendTransaction` RPC. The math CAPTCHA provides light spam friction on the client side.

However, **five critical-severity issues** and several high/medium gaps must be addressed before the faucet is suitable for public testnet use.

### Issue Counts

| Severity | Count | Fixed |
|---|---|---|
| 🔴 CRITICAL | 5 | 0 |
| 🟠 HIGH | 7 (+2 new) | 0 |
| 🟡 MEDIUM | 6 | 0 |
| 🟢 LOW | 6 (+1 new) | 0 |
| **Total** | **24** | **0** |

---

## 2. File Inventory

| File | Lines | Role |
|---|---|---|
| `faucet/index.html` | 253 | Main UI — form, stats, recent-requests table, footer |
| `faucet/faucet.js` | 149 | Client-side logic — form submit, captcha, RPC calls |
| `faucet/faucet.test.js` | 210 | Node.js unit tests for Phase 16 audit fixes |
| `faucet/src/main.rs` | 686 | Rust/Axum backend — rate limiting, keypair, transfer |
| `faucet/Cargo.toml` | 18 | Rust dependencies |
| `faucet/shared-config.js` | 42 | URL config (dev vs production) for all frontends |
| `faucet/shared/utils.js` | 507 | `escapeHtml`, `moltRpcCall`, base58, chain status bar |
| `faucet/shared/wallet-connect.js` | (unread) | Loaded in `shared/` but **not referenced by index.html** |
| `faucet/faucet.css` | (unread — CSS only) | Faucet-specific styles |
| `contracts/moltcoin/src/lib.rs` | 494 | MoltCoin WASM ERC-20 token contract (reference only) |

---

## 3. HTML Audit (`index.html`)

### 3.1 Structure Overview

```
<head>
  charset/viewport, title, meta description
  favicon.ico
  shared-base-styles.css, shared-theme.css, faucet.css
  Google Fonts (Inter, JetBrains Mono)
  Font Awesome 6.5.1 (CDN, cdnjs.cloudflare.com)
<body>
  <nav> — Logo + menu + navToggle button
  <section.faucet-hero> — Badge, H1, subtitle
  <section.section>
    .stats-grid (3 stat-cards — hardcoded values)
    .faucet-card
      <form id="faucetForm">
        input#address (text, required)
        input#captcha (number, required)
        #captchaQuestion > span#num1 + span#num2
        button#submitBtn (submit)
      #successMessage.message-success.hidden
        <div> (innerHTML target for success text)
      #errorMessage.message-error.hidden
        <div id="errorText"> (textContent target)
    .info-grid (3 static info cards)
    .recent-section
      table > tbody#recentRequests
  <footer>
    Logo, links to docs/tools/community
    .chain-status-bar
      span#chainDot
      span#chainBlockHeight
      span#chainLatency
  <script src="shared/utils.js">
  <script src="shared-config.js">
  <script src="faucet.js">
```

### 3.2 Element Inventory

| Element ID / Selector | Type | Used by JS? | Notes |
|---|---|---|---|
| `#navToggle` | button | ✅ `faucet.js:21` | Mobile toggle |
| `.nav-menu` | ul | ✅ `faucet.js:22` | Toggle nav |
| `#address` | text input | ✅ `faucet.js:46` | Wallet address |
| `#captcha` | number input | ✅ `faucet.js:47` | Math answer |
| `#captchaQuestion` | div | ✅ indirect | Contains num1/num2 spans |
| `#num1` | span | ✅ `faucet.js:9` | `generateCaptcha()` writes operand |
| `#num2` | span | ✅ `faucet.js:10` | `generateCaptcha()` writes operand |
| `#submitBtn` | submit button | ✅ `faucet.js:48,69,97` | Disabled during submit |
| `#faucetForm` | form | ✅ `faucet.js:43` | Submit listener |
| `#successMessage` | div | ✅ `faucet.js:49,77,82` | Hidden/shown on success |
| `#successMessage > div` | div | ✅ `faucet.js:83` | `innerHTML` target for result |
| `#errorMessage` | div | ✅ `faucet.js:50,100,103` | Hidden/shown on error |
| `#errorText` | div | ✅ `faucet.js:101` | `textContent` for error msg |
| `#recentRequests` | tbody | ✅ `faucet.js:117` | Prepend new request rows |
| `.stat-card:first-child .stat-value` | div | ✅ `faucet.js:32` | `updateStats()` overwrites |
| `#chainDot` | span | ✅ `shared/utils.js:~485` | Auto-wired chain status |
| `#chainBlockHeight` | span | ✅ `shared/utils.js:~484` | Auto-wired chain status |
| `#chainLatency` | span | ✅ `shared/utils.js:~486` | Auto-wired chain status |

**All JS-referenced element IDs exist in HTML. No undefined-element silent failures.** ✅

### 3.3 Hardcoded Values in HTML

| Element | Hardcoded Value | Backend Actual Default | Match? |
|---|---|---|---|
| `.stat-card:first-child .stat-value` | `100 MOLT` | `MAX_PER_REQUEST=100` | ✅ |
| Cooldown stat card | `24 Hours` | `COOLDOWN_SECONDS=60` (60 seconds) | ❌ **MISMATCH** |
| Daily Limit stat card | `100 MOLT / IP` | `DAILY_LIMIT_PER_IP=100` | ✅ |

> ⚠️ **Issue #7**: The "Cooldown" stat card is hardcoded to "24 Hours" in HTML, but `main.rs` defaults `COOLDOWN_SECONDS` to **60 seconds** (1 minute), not 86 400. This is misleading. Users who see "24 Hours" may be confused that they can re-request quickly.

### 3.4 Accessibility & Responsive Notes

- `<label for="address">` correctly links to `<input id="address">` ✅
- `<label for="captcha">` correctly links to `<input id="captcha">` ✅
- No `aria-live` region on `#successMessage` / `#errorMessage` — screen readers may not announce toast messages ⚠️
- Mobile nav toggle is present (`#navToggle`) ✅
- `<img>` tags have `alt` attributes ✅
- Font Awesome icons are presentational (`<i>` tags) with no `aria-hidden="true"` ⚠️ (minor accessibility gap)

---

## 4. JS Feature Audit (`faucet.js`)

### 4.1 Script Load Order

```html
<script src="shared/utils.js">   → defines escapeHtml, moltRpcCall, bs58encode/decode, auto-wires chainStatusBar
<script src="shared-config.js">  → defines window.MOLT_CONFIG
<script src="faucet.js">         → uses escapeHtml (from utils.js ✅), MOLT_CONFIG
```

### 4.2 Function Inventory

| Function | Lines | What It Does | Issues |
|---|---|---|---|
| `generateCaptcha()` | 7–13 | Sets `#num1`, `#num2` spans; returns `num1+num2` | ⚠️ Trivially bypassable (see §7) |
| `updateStats()` | 28–36 | `GET /health`; if OK sets first stat card to `100 MOLT` | ⚠️ No-op (see §4.4) |
| Form `submit` listener | 43–98 | Validates address + captcha; POSTs to `/faucet/request`; renders result | ❌ See issues below |
| `showError(message)` | 101–108 | Sets `#errorText.textContent`; renews captcha | ✅ Safe (textContent) |
| `addRecentRequest(address, amount, signature)` | 111–130 | Prepends row to `#recentRequests` tbody | ✅ Escaped (F16.1) |
| Nav toggle listener | 20–24 | Toggles `.active` on `.nav-menu` and `#navToggle` | ✅ |

### 4.3 RPC / API Calls

| Call | Method | Endpoint | Params Sent | Response Fields Used |
|---|---|---|---|---|
| `updateStats()` | `GET` | `${FAUCET_API}/health` | none | `resp.ok` only |
| Form submit | `POST` | `${FAUCET_API}/faucet/request` | `{ address, amount: 100 }` | `data.success`, `data.signature`, `data.amount`, `data.error` |

**No direct blockchain RPC calls from faucet.js.** All chain interaction is delegated to the Rust backend. ✅

The chain status bar (from `shared/utils.js`) calls:

| Call | Method | Endpoint | Purpose |
|---|---|---|---|
| `moltRpcCall('getSlot', [])` | `POST` | `getMoltRpcUrl()` | Chain tip for status bar |

### 4.4 `updateStats()` — No-Op Analysis

```javascript
async function updateStats() {
    try {
        const resp = await fetch(`${FAUCET_API}/health`);
        if (resp.ok) {
            document.querySelector('.stat-card:first-child .stat-value').textContent = MOLT_PER_REQUEST + ' MOLT';
        }
    } catch (e) {}
}
```

- `/health` handler returns `(StatusCode::OK, "OK")` — a plain text response, **not JSON**.
- The JS never parses the body — it only checks `resp.ok`.
- If `resp.ok`, it sets the first stat card to `"100 MOLT"` — the **identical value already hardcoded in HTML**.
- The endpoint does not expose backend config values (`cooldown_seconds`, `daily_limit_per_ip`, `max_per_request`).
- **Result**: This call is a pure liveness check with no visible effect on UI. The cooldown and daily limit stat cards are **never updated from backend** and remain hardcoded.

### 4.5 Form Submit — Detailed Analysis

```
1. Prevents default form submission ✅
2. Reads #address, #captcha values ✅
3. Hides previous success/error messages ✅
4. Validates address: length 32–44, base58 regex ✅ (F16.4)
5. Validates captcha against captchaAnswer ✅ (client-side only — see §7)
6. Disables submitBtn with spinner ✅
7. POSTs { address, amount: 100 } to /faucet/request ✅
8. On success:
   - escapeHtml(data.signature || '') → safeSig ✅ (F16.2)
   - escapeHtml(String(data.amount)) → safeAmount ✅ (F16.2)
   - encodeURIComponent on sig/address/amount in explorer href ✅ (F16.2)
   - Sets successMessage .innerHTML ⚠️ (innerHTML used, but values are escaped)
9. On failure: textContent for errorText ✅
10. Re-enables submitBtn in finally ✅
11. Calls addRecentRequest with escaped values ✅ (F16.1)
```

**Issue**: Response field `data.amount` is `Option<u64>` on the backend. If backend returns `success: true` but omits `amount` (would not happen with current code, but defensive coding gap), `String(undefined)` → `"undefined MOLT sent"`.

### 4.6 `addRecentRequest()` — XSS Safety

```javascript
const shortAddress = escapeHtml(`${address.slice(0, 8)}...${address.slice(-4)}`);
const safeAmount = escapeHtml(String(amount));
```

Address and amount are escaped before `innerHTML` injection. ✅ F16.1 fix verified.

### 4.7 Chain Status Bar

The chain status bar auto-wire IIFE in `shared/utils.js` calls `moltRpcCall('getSlot', [])`. The `getMoltRpcUrl()` function checks:

```javascript
if (window.moltConfig && window.moltConfig.rpcUrl) ...      // not set
if (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) ... // not set
if (window.moltExplorerConfig && window.moltExplorerConfig.rpcUrl) ... // not set
return 'http://localhost:8899';  // always falls through to this
```

The faucet page sets `window.MOLT_CONFIG` (uppercase), not any of the checked names. **The chain status bar will always use `http://localhost:8899`** — correct in local dev, broken in any deployed environment. ⚠️

### 4.8 `captchaAnswer` Exposure

```javascript
let captchaAnswer = generateCaptcha();
```

`captchaAnswer` is a **module-level variable** accessible from the browser console. The CAPTCHA operands (`#num1`, `#num2`) are plaintext DOM elements. Any bot can:
1. Read `document.getElementById('num1').textContent` and `num2`, add them, and submit.
2. Or simply read `captchaAnswer` from the console.

This is expected for a trivial math CAPTCHA and is acceptable for a light defense layer only if the **backend has robust rate limiting** as its primary defense. The backend does have rate limiting, but it is vulnerable to X-Forwarded-For spoofing (see §7).

### 4.9 No `console.log` Leaking Private Data

Confirmed: no `console.log` statements in `faucet.js`. ✅

### 4.10 No WebSocket Usage

Confirmed: `faucet.js` uses no WebSocket. Chain status uses polling via `setInterval`. ✅

---

## 5. Rust Backend Audit (`src/main.rs`)

### 5.1 Exists and Is Active

The Rust backend **exists and is the primary airdrop mechanism**. The JS frontend calls it at `${FAUCET_API}/faucet/request`. This is a good architecture — rate limiting and token transfer are server-side.

### 5.2 Routes

| Method | Path | Handler | Description |
|---|---|---|---|
| `POST` | `/faucet/request` | `faucet_request_handler` | Primary airdrop endpoint |
| `GET` | `/faucet/airdrops` | `list_airdrops_handler` | List airdrops, optionally filtered by `?address=X&limit=N` |
| `GET` | `/faucet/airdrop/:sig` | `get_airdrop_handler` | Fetch single airdrop record by synthetic sig |
| `GET` | `/health` | `health_handler` | Returns `"OK"` (plain text) |

### 5.3 Rate Limiting

The backend implements **dual-axis rate limiting**: per-address AND per-IP.

```rust
struct RateLimiter {
    ip_usage: HashMap<String, (u64 /* last_ts */, u64 /* molt_today */, u64 /* day_start */)>,
    address_requests: HashMap<String, u64 /* last_ts */>,
}
```

- **Per-address**: Address cannot request again within `COOLDOWN_SECONDS` (default: 60 s).
- **Per-IP**: Daily MOLT total for an IP cannot exceed `DAILY_LIMIT_PER_IP` (default: 100).
- L6 fix: `cleanup_stale()` evicts entries older than 24 h on every check (prevents unbounded memory growth ✅).
- L6 fix: In-memory airdrop history capped to 10 000 entries ✅.

**Critical** — X-Forwarded-For IP extraction:

```rust
let client_ip = headers
    .get("x-forwarded-for")
    .and_then(|v| v.to_str().ok())
    .map(|s| s.split(',').next().unwrap_or("unknown").trim().to_string())
    // ...
    .unwrap_or_else(|| "localhost".to_string());
```

The backend trusts the `X-Forwarded-For` header unconditionally (takes the first IP). **Without a reverse proxy that overwrites this header**, any client can pass `X-Forwarded-For: 1.2.3.4` and change their apparent IP, bypassing per-IP rate limits. ❌

**Additional**: when no `X-Forwarded-For` or `X-Real-Ip` header is present (direct connection, docker-compose dev), `client_ip` falls through to `"localhost"`. This means **all direct-connection users share a single "localhost" IP rate limit bucket** — one user exhausting their 100 MOLT blocks all other directly-connected users.

### 5.4 CAPTCHA Verification

**There is NO CAPTCHA verification server-side.** The backend only validates:
1. Address format (valid base58, decodes to 32-byte pubkey)
2. Amount (1 ≤ amount ≤ max_per_request)
3. Rate limit (per-address and per-IP)

The client-side math puzzle (`num1 + num2`) is trivially bypassable and provides no constraint against automated requests at the per-request level. Rate limiting is the only real bot defense. ⚠️

### 5.5 Airdrop Mechanism — `send_faucet_transfer()`

```
1. Resolve recipient Pubkey from base58 ✅
2. Fetch recent blockhash: POST {"method":"getRecentBlockhash"} ✅
3. Build instruction:
   program_id = [0u8; 32] (system program) ✅
   accounts   = [faucet_pubkey, recipient] ✅
   data       = [0x00] ++ amount_shells.to_le_bytes() (8 bytes) ✅
4. Build Message { instructions: [ix], recent_blockhash } ✅
5. Serialize message with Message::serialize() ✅
6. Sign with faucet keypair ✅
7. Serialize Transaction with bincode ✅
8. Base64-encode and submit via sendTransaction RPC ✅
9. Extract result hex from send_data["result"] ✅
```

The transfer instruction is correctly constructed for a native MOLT system-program transfer (opcode `0x00`, amount in shells). This does NOT interact with the MoltCoin WASM contract — it transfers **native MOLT**, which is correct for a testnet faucet.

### 5.6 Synthetic Transaction Signature

After a successful `sendTransaction`, the server generates a **local synthetic signature**:

```rust
let sig = format!("airdrop-{}", timestamp_ms);
```

This is **not** the transaction signature returned by `send_data["result"]` from the RPC node. The actual on-chain transaction ID (`sig_hex`) is used only in a log message:

```rust
Ok(format!("{} MOLT transferred to {} (sig: {})", amount_molt, recipient_address, &sig_hex[..16.min(sig_hex.len())]))
```

The synthetic `airdrop-{ts}` signature is stored in the airdrop history and returned to the frontend. The frontend then builds an explorer link:

```javascript
`../explorer/transaction.html?sig=${encodeURIComponent(data.signature)}&to=...`
```

This will create a link like `transaction.html?sig=airdrop-1740614400000` — **the explorer cannot look up this non-standard identifier**. It should use the actual on-chain transaction hex returned by `sendTransaction`. ❌

### 5.7 Faucet Wallet Funding

The backend calls `send_faucet_transfer` without first checking the faucet wallet's MOLT balance. If the faucet wallet is empty or underfunded:
1. The rate limit is **already consumed** before the transfer is attempted.
2. `sendTransaction` will fail with an on-chain error ("insufficient funds").
3. The user sees "Airdrop failed: sendTransaction failed: ..." and cannot retry for `COOLDOWN_SECONDS`.

There is no pre-flight balance check, no alerting, and no admin notification when the faucet wallet runs out of funds. ❌

### 5.8 Keypair Handling

- `FAUCET_KEYPAIR` env var supports JSON byte array, JSON object (`secret_key`/`privateKey`/`seed`), and raw hex string formats ✅
- Auto-generates a new keypair if not configured ✅
- I-4 fix: Sets `0o600` file permissions on the generated keypair file ✅
- The generated keypair is written to `faucet-keypair.json` in the process working directory ✅

**Issue**: `docker-compose.yml` does not mount `FAUCET_KEYPAIR` or a volume for `faucet-keypair.json`. On container restart, a new keypair is generated, losing the faucet wallet address. Any MOLT pre-funded to the old address becomes inaccessible. ❌

### 5.9 Mainnet Guard

```rust
if config.network == "mainnet" {
    panic!("❌ Faucet cannot run on mainnet!");
}
```

Hard panic prevents mainnet runs ✅.

### 5.10 CORS Configuration

```rust
CorsLayer::new()
    .allow_origin([
        "https://faucet.moltchain.io".parse::<HeaderValue>().unwrap(),
        "https://moltchain.io".parse::<HeaderValue>().unwrap(),
        "http://localhost:3003".parse::<HeaderValue>().unwrap(),
        "http://localhost:3000".parse::<HeaderValue>().unwrap(),
    ])
```

I-5 fix: CORS is restricted to specific origins (not wildcard `*`) ✅.

**Issue**: The faucet frontend is configured for `http://localhost:9100` (dev) or served from the same origin in production. However:
- `http://localhost:9100` is **not in the CORS allow-list**.
- `http://localhost:3003` and `http://localhost:3000` are allowed, but those are the explorer and possibly website ports.
- Opening `index.html` directly as a `file://` URL sends `Origin: null` which is not in the allow-list.
- This means **any browser-based dev testing from the faucet's own dev server port (9100) will be blocked by CORS**. ❌

### 5.11 Port — Docker-Compose vs Config Mismatch

| Source | Port |
|---|---|
| `shared-config.js` faucet dev URL | `http://localhost:9100` |
| `docker-compose.yml` `ports` | `9101:9101` |
| `docker-compose.yml` `PORT` env | `9101` |
| `main.rs` default `PORT` | `9100` |

When running via docker-compose, `faucet.js` constructs `FAUCET_API = 'http://localhost:9100'` but the container is listening on `9101`. **All API calls from the frontend will fail with ECONNREFUSED.** ❌

---

## 6. Contract Integration Audit

### 6.1 Which Contract Does the Faucet Use?

The faucet does **NOT** call the MoltCoin ERC-20 WASM contract (`contracts/moltcoin/src/lib.rs`). It issues a **native system-program transfer** from the faucet wallet's MOLT balance.

| Aspect | Faucet Backend | MoltCoin Contract |
|---|---|---|
| Program ID | `[0u8; 32]` (system program) | Deployed WASM address (unknown) |
| Instruction opcode | `0x00` (native transfer) | ABI-encoded WASM calls |
| Token type | Native MOLT (chain currency) | ERC-20 MOLT token balance |
| Amount unit | shells (1 MOLT = 1e9 shells) | shells (1 MOLT = 1e9 shells) ✅ |

This is the correct design: the faucet distributes native MOLT for paying gas, not ERC-20 tokens.

### 6.2 Instruction Byte Layout — Native Transfer

**Backend constructs:**
```rust
let mut ix_data = vec![0x00u8];       // opcode: Transfer
ix_data.extend_from_slice(&amount_shells.to_le_bytes()); // 8 bytes LE u64
// Total: 9 bytes
```

**Expected by MoltChain system program (from `core/src/` convention):**
```
[0x00]                    ← instruction type (Transfer)
[u64 LE 8 bytes]          ← amount in shells
```

**Accounts**: `[faucet_pubkey (signer), recipient]`

This matches the standard MoltChain native transfer format. ✅

### 6.3 MoltCoin Contract Facts (Reference)

| Property | Value |
|---|---|
| Token name | MoltCoin |
| Symbol | MOLT |
| Decimals | 9 |
| Initial supply | 1,000,000 MOLT (1M × 1e9 shells) |
| Max supply cap | 10,000,000,000 MOLT (10B) |
| Mint authority | Owner (set at initialize) |
| Reentrancy guard | ✅ (REENTRANCY_KEY in storage) |
| Transfer auth check | ✅ `get_caller() == from` (F1.8a) |
| Burn auth check | ✅ `get_caller() == from` (F1.8b) |
| transfer_from | ✅ (added in P2 audit) |
| Re-initialization guard | ✅ (rejects if owner already set) |

The faucet does not interact with any of these functions. The distribution is of **native MOLT**, not ERC-20 contract tokens.

### 6.4 Frontend Amount Encoding

```javascript
// faucet.js
body: JSON.stringify({ address, amount: MOLT_PER_REQUEST }) // { amount: 100 }
```

```rust
// main.rs FaucetRequest
struct FaucetRequest { address: String, amount: u64 }
// then:
let amount_shells = amount_molt * 1_000_000_000;
```

JSON `100` → `u64` `100` → `100_000_000_000` shells. ✅ Correct decimal handling.

---

## 7. Security Audit

### 7.1 Can Anyone Drain the Faucet?

**Partially yes.** Key attack vectors:

| Vector | Severity | Protected? |
|---|---|---|
| Repeated requests from same address | ✅ Blocked by per-address cooldown | Protected |
| Repeated requests from different addresses, same IP | ✅ Blocked by per-IP daily limit | Protected |
| Repeated requests with spoofed `X-Forwarded-For` | ❌ Not protected | **CRITICAL** |
| Scripted requests bypassing math CAPTCHA | ⚠️ Trivial to bypass | No server defense |
| VPN/proxy hopping to change IP | ❌ Not protected | Client-side only |
| Multiple users behind same NAT | ⚠️ Rate-limited as single IP | False positive risk |

**Bottom line**: An attacker behind a VPN/proxy pool can enumerate IPs and drain at 100 MOLT/IP/day. This is expected for testnet faucets, but should be documented.

### 7.2 Rate Limiting

- **Per-address cooldown**: ✅ 60 s default (NOT 24 h as shown in UI — mismatched)
- **Per-IP daily cap**: ✅ 100 MOLT/IP/day
- **Global rate limit**: ❌ Not implemented — a distributed attack across many IPs faces no global cap
- **Cleanup**: ✅ L6 fix prevents memory exhaustion

### 7.3 CAPTCHA

- **Client-side math puzzle only**: trivially readable from DOM/JS variables
- **No server-side CAPTCHA verification**: ❌ Backend does not accept/verify any CAPTCHA token
- **Acceptable for testnet**: The rate limiter is the real defense; the CAPTCHA is friction, not security

### 7.4 Auth — Who Can Call the Airdrop?

Anyone can call `POST /faucet/request` directly (bypassing HTML entirely):

```bash
curl -X POST http://localhost:9100/faucet/request \
     -H 'Content-Type: application/json' \
     -d '{"address":"11111111111111111111111111111111","amount":100}'
```

No authentication token / API key is required. The only protection is:
1. Address rate limiting (per address, 60 s)
2. IP rate limiting (100 MOLT/day, spoofable)
3. Max amount enforcement (≤ `MAX_PER_REQUEST`)

For a testnet faucet this is standard. Document as expected behavior.

### 7.5 XSS

- `showError()` uses `textContent` ✅
- `addRecentRequest()` uses `escapeHtml()` on all dynamic values ✅ (F16.1)
- Success message uses `escapeHtml()` on amount and signature, `encodeURIComponent()` on href params ✅ (F16.2)
- No `eval()`, `document.write()`, or `innerHTML` with raw user data ✅

### 7.6 Faucet Keypair Security

- File permissions set to `0o600` on generated keypair ✅ (I-4)
- Keypair not committed to repo (no plaintext seed in code) ✅
- Not mounted as a docker volume — **regenerated on container restart** ❌

### 7.7 Airdrop Persistence

- Airdrop records JSON-serialized to `airdrops.json` ✅
- Not in a docker volume — **lost on container restart** ❌

---

## 8. Issues Found

| # | Severity | Category | Description | File:Line | Fix |
|---|---|---|---|---|---|
| 1 | 🔴 CRITICAL | Config / Port | `shared-config.js` dev faucet URL is `http://localhost:9100` but `docker-compose.yml` maps the faucet container to port `9101` (both host and container). All API calls fail with ECONNREFUSED when running via docker-compose. | [shared-config.js:10](shared-config.js#L10), [docker-compose.yml:39-43](../docker-compose.yml#L39-L43) | Change `shared-config.js` faucet dev URL to `http://localhost:9101` to match docker-compose, OR change docker-compose to map port `9101:9100` so the container's default `9100` is reachable on `9101`. Consistent choice: update `shared-config.js` to `9101` and all references. |
| 2 | 🔴 CRITICAL | Synthetic Signatures / Explorer | `send_faucet_transfer()` returns the real RPC transaction ID in a log message but stores a synthetic `airdrop-{timestamp_ms}` key as the "signature". The explorer link `transaction.html?sig=airdrop-…` will never resolve to a real on-chain transaction. | [src/main.rs:371-385](src/main.rs#L371), [src/main.rs:393-398](src/main.rs#L393), [faucet.js:77-80](faucet.js#L77) | In `send_faucet_transfer()`, return `sig_hex` (the real transaction ID from `send_data["result"]`) directly. Store it in `AirdropRecord.signature`. The frontend explorer link will then resolve correctly. |
| 3 | 🔴 CRITICAL | Security / Rate Limiting | `X-Forwarded-For` header is trusted unconditionally for IP identification. Without a reverse proxy overwriting this header, any client can pass an arbitrary IP and bypass per-IP daily limits. | [src/main.rs:277-290](src/main.rs#L277) | Deploy behind a reverse proxy (nginx/Traefik) configured to overwrite `X-Forwarded-For`. Add a `TRUSTED_PROXY` env var; only honor the header when the connection is from a trusted proxy IP. |
| 4 | 🔴 CRITICAL | Faucet Wallet / Balance | The backend does not check the faucet wallet's MOLT balance before issuing a transfer. When the wallet is empty, `sendTransaction` fails server-side, but the rate limit slot is already consumed — the user cannot retry for `COOLDOWN_SECONDS`. Also, there is no admin alert for low balance. | [src/main.rs:334-340](src/main.rs#L334) | Before calling `send_faucet_transfer`, fetch the faucet wallet balance via `getBalance` RPC. If balance < `amount_shells`, return a 503 with `{"success":false,"error":"Faucet temporarily empty — check back soon"}` **without consuming the rate limit slot**. Add Prometheus/alerting metrics or a `/faucet/status` endpoint exposing wallet balance. |
| 5 | 🔴 CRITICAL | Docker / Docker-Compose | `docker-compose.yml` does not mount a volume for `FAUCET_KEYPAIR` or `faucet-keypair.json`. On container restart, a new keypair is auto-generated, meaning the old faucet wallet address (and any pre-funded MOLT) becomes inaccessible. Similarly, `airdrops.json` is not in a named volume and is lost on restart. | [docker-compose.yml:31-48](../docker-compose.yml#L31) | Add to docker-compose faucet service: `volumes: - faucet-data:/app/data` (or similar) and set `FAUCET_KEYPAIR=/app/data/faucet-keypair.json` and `AIRDROPS_FILE=/app/data/airdrops.json` to persist across restarts. |
| 6 | 🟠 HIGH | CORS | The CORS allow-list (`https://faucet.moltchain.io`, `https://moltchain.io`, `http://localhost:3003`, `http://localhost:3000`) does not include `http://localhost:9100` (the faucet's own dev port) or `http://localhost:9101` (docker port). Any dev running the frontend from file:// or the faucet dev server will be CORS-blocked. | [src/main.rs:230-245](src/main.rs#L230) | Add `http://localhost:9100` and `http://localhost:9101` to the CORS allow-list. For local dev flexibility, consider reading allowed origins from a `CORS_ORIGINS` env var. |
| 7 | 🟠 HIGH | UI Mismatch / Cooldown | The "Cooldown" stat card is hardcoded to "24 Hours" in the HTML, but `main.rs` defaults `COOLDOWN_SECONDS=60` (1 minute). Users expect a 24-hour cooldown; they will be confused when they can re-request after 60 seconds, or when an operator sets a longer cooldown and the UI doesn't reflect it. | [index.html:72](index.html#L72), [src/main.rs:167](src/main.rs#L167) | Expose a `GET /faucet/config` endpoint returning `{ max_per_request, cooldown_seconds, daily_limit_per_ip }`. On page load, fetch this and dynamically update all three stat cards. Alternatively, align the default `COOLDOWN_SECONDS` to 86400 (24 h) if that is the intended policy. |
| 8 | 🟠 HIGH | Chain Status Bar / RPC | The chain status bar in `shared/utils.js` calls `getMoltRpcUrl()` which checks `window.moltConfig`, `window.moltMarketConfig`, `window.moltExplorerConfig`. The faucet sets `window.MOLT_CONFIG` (different name/case). The fallback is always `http://localhost:8899`. In production, the status bar targets localhost — the bar will show "Reconnecting…" in production. | [shared/utils.js:316-322](shared/utils.js#L316), [shared-config.js:1-42](shared-config.js#L1) | Either: (a) add `window.moltConfig = { rpcUrl: MOLT_CONFIG.rpc || 'http://localhost:8899' }` at the bottom of `shared-config.js`, or (b) update `getMoltRpcUrl()` to also check `window.MOLT_CONFIG.rpc`. Add a `rpc` key to `MOLT_CONFIG`. |
| 9 | 🟠 HIGH | Rate Limit / Direct Client IP | When there is no reverse proxy (docker-compose default with no nginx), direct TCP connections produce `client_ip = "localhost"`. All direct users share the same IP bucket — one user consuming 100 MOLT blocks all other direct-connection users until the next day. | [src/main.rs:291-297](src/main.rs#L291) | Fall back to the actual remote socket address when no proxy headers are present. Axum provides `ConnectInfo<SocketAddr>` for this. Add `ConnectInfo<SocketAddr>` as an extractor, use its IP when no `X-Forwarded-For` or `X-Real-Ip` is present. |
| 10 | 🟠 HIGH | Stats Not Synced from Backend | `updateStats()` fetches `/health` (returns `"OK"`, no JSON) and only overwrites the first stat card with a constant already in HTML. The cooldown and daily-limit cards are never dynamically updated. Operators can change env vars and redeploy, but the UI always shows the hardcoded HTML values. | [faucet.js:28-36](faucet.js#L28), [src/main.rs:261-270](src/main.rs#L261) | Implement `GET /faucet/config` (see issue #7 fix). Update `updateStats()` to call this endpoint and update all three stat cards, plus a "Faucet Balance" card. |
| 11 | 🟡 MEDIUM | Explorer Link | The explorer link `../explorer/transaction.html?sig=airdrop-{ts}` passes a synthetic timestamp-based key, not a real hex transaction ID. This is a consequence of issue #2. Even if fixed, the explorer link assumes a relative path which breaks if the apps are served from different origins. | [faucet.js:78-80](faucet.js#L78) | After fixing issue #2, build the explorer URL using `MOLT_CONFIG.explorer` as the base, not a relative path: ``${MOLT_CONFIG.explorer}/transaction.html?sig=${encodeURIComponent(realSig)}``. |
| 12 | 🟡 MEDIUM | Historical Airdrops Not Loaded | The "Recent Requests" table defaults to "No recent requests yet" and is only populated by the current browser session's requests. The backend exposes `GET /faucet/airdrops` but `faucet.js` never calls it on page load. Users see no history across sessions. | [faucet.js:111-130](faucet.js#L111), [src/main.rs:570-597](src/main.rs#L570) | On DOMContentLoaded, call `GET ${FAUCET_API}/faucet/airdrops?limit=10` and populate `#recentRequests` with the response. |
| 13 | 🟡 MEDIUM | No Fetch Timeout | The form submit `fetch` to `/faucet/request` has no timeout. If the backend hangs, `#submitBtn` remains disabled indefinitely and the user cannot retry without reloading. | [faucet.js:65-96](faucet.js#L65) | Wrap the fetch in `Promise.race` with an `AbortController` timeout of 15–30 seconds. On timeout, show an error like "Request timed out. Please try again." and re-enable the button. |
| 14 | 🟡 MEDIUM | `data.amount` May Be Undefined | If the backend omits `amount` from the JSON response (unexpected state), `String(data.amount)` → `"undefined"`, and the success banner shows "undefined MOLT sent to your address". | [faucet.js:75](faucet.js#L75) | Default: `const safeAmount = escapeHtml(String(data.amount ?? MOLT_PER_REQUEST));`. |
| 15 | 🟡 MEDIUM | No Faucet Balance Display | There is no UI element showing the faucet wallet balance, so users cannot tell if the faucet has funds. When the faucet is drained, all requests silently fail with a backend error. | [index.html:57-84](index.html#L57) | Add a "Faucet Balance" stat card. Populate it from a `/faucet/status` endpoint that returns the balance by calling `getBalance` on the faucet wallet's pubkey. |
| 16 | 🟡 MEDIUM | `updateStats()` Called Before DOM Events | `updateStats()` is called synchronously at module evaluation time (line 37). It accesses `document.querySelector('.stat-card:first-child .stat-value')`. Since the `<script>` is at the bottom of `<body>` the DOM is already painted, so it works — but only accidentally. The function is `async` and the await is inside the try block; if the fetch resolves after some navigation, the querySelector could return null. | [faucet.js:30-36](faucet.js#L30) | Call `updateStats()` inside a `DOMContentLoaded` listener for robustness, and add a null guard: `const el = document.querySelector(...); if (el) el.textContent = ...` |
| 17 | 🟢 LOW | Nav Links Hardcoded | Nav links use relative HTML paths (`../wallet/index.html`, `../explorer/index.html`) rather than `data-molt-app` attributes. The `MOLT_CONFIG` URL resolution in `shared-config.js` only resolves `<a data-molt-app="X">` links. Nav links break when apps are served on different ports or subdomains. | [index.html:28-33](index.html#L28) | Update nav links to use `data-molt-app="wallet"` and `data-molt-path=""` (or equivalent) so they resolve correctly across environments. |
| 18 | 🟢 LOW | Footer Links to .md Files | Footer links to `../docs/README.md`, `../docs/foundation/VISION.md`, etc. are markdown files. Browsers render them as plain text (or download them). They should link to rendered HTML documentation. | [index.html:217-225](index.html#L217) | Update footer links to point to rendered HTML docs (`../developers/index.html`, etc.) or update the docs system to serve HTML. |
| 19 | 🟢 LOW | `shared/wallet-connect.js` Unused | `faucet/shared/wallet-connect.js` exists but is never `<script>`-loaded by `index.html`. It takes up space and may cause confusion. | [index.html](index.html) | Remove from the faucet's `shared/` directory or add it to a `.gitignore`-style note. If wallet-connect is intentional (future feature), add a comment in the HTML. |
| 20 | 🟢 LOW | Comment Says Port 9100 | `faucet.js` line 2 comment: "Rust/axum on port 9100". Docker-compose runs on 9101. Minor misleading comment. | [faucet.js:2](faucet.js#L2) | Update comment to match actual deployment port, or reference the `PORT` env var. |
| 21 | 🟢 LOW | No `aria-live` on Status Messages | `#successMessage` and `#errorMessage` have no `aria-live="polite"` attribute. Screen readers will not announce faucet results. Font Awesome `<i>` icons lack `aria-hidden="true"`. | [index.html:116-128](index.html#L116) | Add `aria-live="polite"` to `#successMessage` and `#errorMessage`. Add `aria-hidden="true"` to decorative `<i>` tags. |
| 22 | 🟠 HIGH | `window.MOLT_CONFIG` never set — `FAUCET_API` always localhost | `shared-config.js` declares `const MOLT_CONFIG = (...)()`. In classic (non-module) browser scripts, `const` at the top level does **not** become a property of `window`. So `window.MOLT_CONFIG` is always `undefined`. `faucet.js:4` reads `(window.MOLT_CONFIG && window.MOLT_CONFIG.faucet) \|\| 'http://localhost:9100'` — the condition is always falsy, meaning **`FAUCET_API` is permanently `localhost:9100` in every environment including production**. The production URL in `shared-config.js` is never used. | [faucet.js:4](faucet.js#L4), [shared-config.js:5](shared-config.js#L5) | Change `faucet.js:4` to use the bare global: `const FAUCET_API = (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG.faucet) \|\| 'http://localhost:9100';`. This is distinct from but compounded by issue #8 (chain status bar also doesn't read `MOLT_CONFIG`). |
| 23 | 🟠 HIGH | Doubled `/faucet` path in production URL | `shared-config.js` production branch sets `faucet: \`${base}/faucet\`` (e.g. `https://moltchain.io/faucet`). But `faucet.js` constructs all API calls by appending the route: `` `${FAUCET_API}/faucet/request` ``, `` `${FAUCET_API}/health` ``. With `FAUCET_API = 'https://moltchain.io/faucet'`, this produces `https://moltchain.io/faucet/faucet/request` — a doubled path that will 404. | [shared-config.js:29](shared-config.js#L29), [faucet.js:32](faucet.js#L32), [faucet.js:80](faucet.js#L80) | Change `shared-config.js` production `faucet` value to `base` (not `${base}/faucet`): `faucet: base`. The backend routes already include the `/faucet/` prefix so the correct URL is `https://moltchain.io/faucet/request`. Note: issue #22 must be fixed simultaneously or this will remain unreachable anyway. |
| 24 | 🟢 LOW | User-visible error message hardcodes wrong port | `faucet.js:111` shows the user: `'Could not reach faucet service. Make sure the faucet backend is running on port 9100.'` Docker-compose runs the backend on port `9101`. Users following this message will look at the wrong port. | [faucet.js:111](faucet.js#L111) | Change the message to omit the hardcoded port or derive it from `FAUCET_API`: `'Could not reach faucet service at ' + FAUCET_API + '. Is the backend running?'`. |

---

## 9. Summary of Recommended Fixes (Priority Order)

### Immediate (Before Any Public Use)

1. **Fix `window.MOLT_CONFIG` → bare `MOLT_CONFIG`** (issue #22): `FAUCET_API` is locked to localhost in all environments — production is broken without this.
2. **Fix doubled `/faucet` path in production config** (issue #23): `shared-config.js` production `faucet` value must be `base`, not `${base}/faucet`.
3. **Fix port mismatch** (issue #1): Align `shared-config.js` dev port with docker-compose (`9101`).
4. **Persist keypair and airdrops** (issue #5): Add docker volume mounts and env vars.
5. **Fix real transaction signature** (issue #2): Store `sig_hex` from `sendTransaction` result, not a synthetic key.
6. **Fix CORS** (issue #6): Add `http://localhost:9100` and `http://localhost:9101` to the allow-list.
7. **Add balance pre-flight** (issue #4): Check faucet wallet balance before rate-limiting.
8. **Fix direct-connection IP** (issue #9): Use `ConnectInfo<SocketAddr>` as fallback for client IP.

### Short-Term (Within a Sprint)

9. **Add `/faucet/config` endpoint** and update `updateStats()` to sync all three stat cards (issues #7, #10).
10. **Align cooldown default** to 86400 s if "24 Hours" is the intended policy (issue #7).
11. **Add fetch timeout** to form submission (issue #13).
12. **Load recent airdrops on page load** from `/faucet/airdrops` (issue #12).
13. **Fix chain status bar** RPC URL namespace (issue #8).
14. **Use `MOLT_CONFIG.explorer` base URL** for explorer links (issue #11).

### Longer-Term

15. Add Prometheus metrics or a `/faucet/status` balance endpoint (issue #15).
16. Document X-Forwarded-For trust requirement; add TRUSTED_PROXY env var (issue #3).
17. Fix user-visible error message port reference (issue #24).
18. Resolve nav link hardcoding (issue #17).
19. Fix footer links to rendered HTML docs (issue #18).
20. Add `aria-live` accessibility attributes (issue #21).

---

---

*Original audit: February 27, 2026 — Re-verified and updated February 27, 2026*
*0 of 21 original issues resolved. 3 new issues added (#22–24).*
