# MoltChain SDK — Production Readiness Audit (Full)

**Date:** February 2025  
**Scope:** JS SDK, Python SDK, Rust SDK, DEX SDK, Main Contract SDK, Shared Wallet-Connect  
**Methodology:** Complete line-by-line review of every source file across all SDK directories

---

## Table of Contents

1. [JS SDK](#1-js-sdk-sdkjs)
2. [Python SDK](#2-python-sdk-sdkpython)
3. [Rust SDK](#3-rust-sdk-sdkrust)
4. [DEX SDK](#4-dex-sdk-dexsdk)
5. [Main Contract SDK](#5-main-contract-sdk-sdksrc)
6. [Shared Wallet-Connect](#6-shared-wallet-connect)
7. [Cross-SDK Consistency](#7-cross-sdk-consistency)
8. [Summary Matrix](#8-summary-matrix)

---

## 1. JS SDK (`sdk/js/`)

### 1.1 `src/index.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Stub | `VERSION` is hardcoded `'0.1.0'` — not read from `package.json`. Will drift. | Low |
| 2 | Dead code | `DEFAULT_RPC_URL` and `DEFAULT_WS_URL` are exported but never consumed by any SDK class (Connection requires explicit URL). | Low |
| 3 | Missing functionality | No re-export of `encodeTransaction`, `encodeMessage`, `hexToBytes`, `bytesToHex` from `bincode.ts` — users who need low-level encoding cannot access them through the main barrel. | Medium |

### 1.2 `src/connection.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Error handling | `rpc()` calls `response.json()` and casts to `any`. If the server returns non-JSON (e.g. HTML error page), `.json()` throws an unhandled rejection. Only the non-OK branch calls `.text()`. | Medium |
| 2 | Type safety | 17 methods return `Promise<any>` (`getStakingStatus`, `getStakingRewards`, `getAccountInfo`, `getTransactionHistory`, `getContractInfo`, `getContractLogs`, `getContractAbi`, `setContractAbi`, `getAllContracts`, `getProgram`, `getProgramStats`, `getPrograms`, `getProgramCalls`, `getProgramStorage`, all NFT methods). No typed return values. | Medium |
| 3 | Security | `setContractAbi()` sends arbitrary `abi: any` directly to RPC with no validation — potential injection vector if RPC handler trusts the payload. | Medium |
| 4 | Missing functionality | No `getProgramAccounts` filtering (e.g., by data size, memcmp). The method returns `result.accounts || []` but the `|| []` masks RPC format differences. | Low |
| 5 | WebSocket | **No reconnect logic.** If the WebSocket disconnects, all subscriptions are silently lost; no auto-reconnect, no event emitter for disconnect. | Critical |
| 6 | WebSocket | **No heartbeat/ping.** Connection can go stale behind NATs/proxies without detection. | High |
| 7 | WebSocket | `connectWs()` has a race condition: if called concurrently, multiple WebSocket instances may be created (the `readyState` check runs before the new WS is fully assigned). | Medium |
| 8 | WebSocket | `on('close')` handler is missing entirely — no cleanup of subscriptions on server-initiated close. | High |
| 9 | Error handling | `subscribe()` has a 5-second hard timeout with no configurability; if the server is slow, subscriptions fail silently. | Medium |
| 10 | API consistency | `sendTransaction()` encodes to base64 — must match the backend's expected encoding. No fallback for hex encoding. | Low |
| 11 | Missing functionality | No `close()` cleanup for the HTTP connection; no `AbortController` for in-flight requests. | Low |
| 12 | Missing functionality | `getValidators()` assumes `result.validators` exists — if the RPC returns a bare array, this throws. | Medium |
| 13 | Missing functionality | No retry logic on transient HTTP errors (5xx, network timeouts). | Medium |
| 14 | Dead code | `PROGRAM ENDPOINTS (DRAFT)` and `NFT ENDPOINTS (DRAFT)` are marked Draft in comments but exported as stable public API. | Low |

### 1.3 `src/keypair.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | `secretKey` (64-byte Ed25519 secret key) is `readonly` but **publicly accessible** — `keypair.secretKey` is exposed to any code holding a Keypair reference. Should use `#secretKey` (private class field). | Critical |
| 2 | Security | `publicKey` raw bytes also public — less dangerous but inconsistent (Python SDK hides `_signing_key`). | Low |
| 3 | Missing functionality | No `fromSecretKey()` factory (for 64-byte format). Cannot reconstruct from exported key. | Low |
| 4 | Missing functionality | No `save()` / `load()` methods — Python SDK has them, JS SDK does not. Cross-SDK inconsistency. | Medium |
| 5 | Missing functionality | No `toJSON()` / serialization method. | Low |

### 1.4 `src/publickey.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Type safety | Constructor accepts `number[]` and converts via `new Uint8Array(value)`. Values outside 0–255 silently wrap modulo 256. No validation. | Low |
| 2 | Missing functionality | No hash / equality support for use as `Map` key — two `PublicKey` objects with the same bytes are not `===`. | Medium |
| 3 | Missing functionality | No `isOnCurve()` check — any 32 bytes are accepted, including non-Ed25519 points. | Low |

### 1.5 `src/transaction.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Type safety | `amount: number` parameter in `transfer()`, `stake()`, `unstake()` is a JavaScript `number`, which loses precision above 2^53. For amounts > ~9 billion shells (9 MOLT), precision loss occurs. Should accept `bigint`. | High |
| 2 | Missing functionality | No multi-signer support — `buildAndSign()` takes a single `Keypair`. No `partialSign()`. | Medium |
| 3 | Missing functionality | No `Instruction.serialize()` or JSON conversion method. | Low |
| 4 | API consistency | System Program ID uses `'11111111111111111111111111111111'` (base58 all-1s = `[0x00; 32]`). Consistent in bytes with Python but visually confusing. | Info |

### 1.6 `src/bincode.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Serialization | `encodeMessage()` encodes blockhash via `hexToBytes()` — assumes blockhash is always hex. If the RPC returns base58 blockhash, this will produce wrong bytes. | High |
| 2 | Serialization | `encodeU64LE` accepts `number | bigint`. For `number`, precision above 2^53 is silently lost before conversion to `BigInt`. | Medium |
| 3 | Dead code | `encodeString()` and `encodeBytes()` are defined but never called anywhere. | Low |
| 4 | Missing functionality | No `decodeTransaction` / `decodeMessage` — only encoding exists, no round-trip capability. | Low |

### 1.7 `package.json`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | `jest` is listed in `scripts.test` but not in `devDependencies`. Running `npm test` will fail. | Medium |
| 2 | Missing functionality | No `eslint`, `prettier`, or any lint tooling. | Low |
| 3 | Missing functionality | No `engines` field — no Node.js version requirement specified. | Low |

### 1.8 `tsconfig.json`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | `lib` does not include `DOM` — `fetch` global is not typed. TypeScript will error on `fetch` without node 18+ types or `--lib DOM`. | Medium |

### 1.9 Test files

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | `test.js` | Minimal smoke test — only tests `getNetworkInfo`. No assertions. | Low |
| 2 | `test-all-features.ts` | Comprehensive but **no unit tests** — all tests require a live validator. No mocks/stubs. | Medium |
| 3 | `test-subscriptions.js` | Uses hardcoded pubkey `6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H` — tests fail in any environment without this specific account. | Low |
| 4 | `test_bincode_format.js` | Good: verifies bincode encoding matches Rust format. Standalone assertions. | Info |
| 5 | `generate_transactions.ts` | Functional — sends real transactions. No error recovery if RPC is down. | Low |

---

## 2. Python SDK (`sdk/python/`)

### 2.1 `moltchain/__init__.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | API consistency | `__version__ = "0.1.0"` hardcoded, duplicates `setup.py` version string. Will drift. | Low |

### 2.2 `moltchain/connection.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Error handling | `_rpc()` checks `"error" in data` but doesn't handle network errors from `httpx` (timeout, connection refused) — they propagate as raw `httpx` exceptions, not SDK-specific errors. | Medium |
| 2 | Error handling | `_handle_ws_messages()` swallows all callback exceptions with `except Exception: pass`. Errors in user callbacks are silently lost. | Medium |
| 3 | WebSocket | **No reconnect logic.** If the WebSocket connection drops, `_handle_ws_messages()` exits on `ConnectionClosed` and all subscriptions are permanently lost. | Critical |
| 4 | WebSocket | **No heartbeat/ping.** | High |
| 5 | WebSocket | `_subscribe()` uses `asyncio.get_event_loop()` — deprecated in Python 3.10+. Should use `asyncio.get_running_loop()`. | Medium |
| 6 | Type safety | `send_transaction()` returns bare `result` — could be a string or dict depending on RPC version. Not typed. | Low |
| 7 | Missing functionality | No `getProgramAccounts` method (JS SDK has it). | Medium |
| 8 | Missing functionality | No `simulateTransaction` method (JS SDK has it). | Medium |
| 9 | Missing functionality | No `getContractAbi` / `setContractAbi` methods (JS SDK has them). | Low |
| 10 | Missing functionality | No retry logic on transient HTTP errors. | Medium |
| 11 | API consistency | `get_validators()` assumes `result["validators"]` — will `KeyError` if RPC returns a list directly. | Medium |
| 12 | API consistency | `get_peers()` assumes `result["peers"]` — same issue. | Medium |
| 13 | Dead code | Marketplace WS subscriptions (`on_market_listings`, `on_market_sales`) present but no corresponding REST query methods. | Low |

### 2.3 `moltchain/keypair.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | `save()` method writes seed to disk as JSON with **plaintext seed bytes** and no encryption. File permissions are not set to 600. | High |
| 2 | Security | `load()` reads `privateKey` field and extracts first 32 bytes as seed — silently accepts any data. No pubkey verification against stored pubkey. | Medium |
| 3 | Missing functionality | No `from_secret_key()` (64-byte format). | Low |
| 4 | Missing functionality | No `to_json()` method. | Low |

### 2.4 `moltchain/publickey.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Type safety | Constructor accepts `list` and converts via `bytes(value)` — values > 255 raise `ValueError` with a poor error message. | Low |
| 2 | Missing functionality | No `isOnCurve()` check. | Low |
| 3 | Positive | `__hash__` and `__eq__` properly implemented — usable in dicts/sets. Better than JS SDK. | Info |
| 4 | Positive | `new_unique()` factory for testing. JS/Rust SDKs lack this. | Info |

### 2.5 `moltchain/transaction.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Type safety | `amount: int` — Python `int` has arbitrary precision, so no overflow risk. Better than JS SDK. | Info |
| 2 | Missing functionality | No multi-signer support. | Medium |
| 3 | API consistency | System Program ID: `PublicKey(b'\x00' * 32)` — resolves to same bytes as JS but uses different construction. | Info |

### 2.6 `moltchain/bincode.py`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Serialization | `_encode_hash()` uses `bytes.fromhex()` — assumes blockhash is hex. Same issue as JS SDK. | High |
| 2 | Dead code | `_encode_string()` is defined but never called. | Low |
| 3 | Positive | Clean separation of `EncodedInstruction` dataclass. | Info |

### 2.7 `setup.py` & `requirements.txt`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | API consistency | `python_requires=">=3.8"` but code uses `asyncio.get_event_loop()` patterns that are deprecated in 3.10 and may break in 3.12+. | Medium |
| 2 | Dead code | `requirements.txt` duplicates `install_requires` from `setup.py`. Should be single source of truth. | Low |

### 2.8 Test & Tool Files

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | `test_bincode.py` | Good standalone unit tests for bincode encoding. Assertions validate Rust format compatibility. | Info |
| 2 | `test_sdk_live.py` | Integration test requiring live validator. No mocks. Assertions use bare `assert`. | Low |
| 3 | `test_websocket_sdk.py` | Near-duplicate of `test_sdk_live.py` WebSocket portion. Redundant file. | Low |
| 4 | `test_websocket_simple.py` | Raw `websockets` test — doesn't use SDK at all. | Low |
| 5 | `send_test_transactions.py` | Reuses same blockhash for all 5 transactions — replay protection may reject duplicates if nonce is not varied. | Medium |
| 6 | `generate_wallet.py` | Prints partial seed hex to console — minor security concern in shared terminal environments. | Low |
| 7 | `adversarial_security_test.py` | Comprehensive attack simulation (1119 lines). Uses raw `urllib` instead of SDK. Port mismatch: uses `http://127.0.0.1:8000` vs SDK default `8899`. | Low |
| 8 | `deep_stress_test.py` | Good stress test. Directly instantiates `nacl.signing.SigningKey` and wraps in `Keypair(signing_key)` — bypasses `Keypair.from_seed()` factory. Relies on internal `_signing_key` field. | Low |
| 9 | `e2e_agent_test.py` | Full agent simulation. Uses custom bincode helpers that duplicate `moltchain.bincode`. Should reuse SDK module. | Low |
| 10 | `financial_attack_test.py` | Uses RPC methods (`createWallet`, `requestAirdrop`, `transfer`, `callContract`) that are NOT in the SDK's `Connection` class — may be validator-only or non-existent endpoints. | Medium |
| 11 | `production_audit.py` | Tests ~50 RPC endpoints. Several (`getRecentTransactions`, `getFeeConfig`, `getRentParams`, etc.) are not exposed in any SDK. | Medium |

---

## 3. Rust SDK (`sdk/rust/`)

### 3.1 `src/lib.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | No WebSocket support at all — JS and Python SDKs have full subscription support. | High |
| 2 | Missing functionality | `wasm` feature declared in `Cargo.toml` but empty / unused. | Low |
| 3 | API consistency | Re-exports `moltchain_core` types (`Account`, `Hash`, `Message`, `Instruction`, `SYSTEM_PROGRAM_ID`, `BASE_FEE`). Tight coupling to core crate. | Info |

### 3.2 `src/client.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Serialization | `send_transaction()` uses `bincode::serialize()` on `moltchain_core::Transaction`. JS/Python SDKs use manual byte-by-byte encoding. If field order differs between `serde(derive)` and manual layout, **cross-SDK transactions are incompatible**. | High |
| 2 | Missing functionality | No `simulateTransaction`, `getProgramAccounts`, or contract ABI methods. | Medium |
| 3 | Missing functionality | No retry/backoff on transient errors. | Medium |
| 4 | Missing functionality | `get_balance()` only reads `result["shells"]` — ignores `molt`, `spendable`, `staked`, `locked`, `reef_*` fields. | Low |
| 5 | Type safety | `get_validators()` returns `Vec<Value>` (untyped JSON). Should have a `Validator` struct. | Medium |
| 6 | Missing functionality | No NFT, marketplace, or analytics methods. | Low |
| 7 | Missing functionality | `ClientBuilder` has no connection pooling configuration. | Low |

### 3.3 `src/error.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Positive | Clean `Error` enum with `thiserror` derive. | Info |
| 2 | Missing functionality | No error code mapping from RPC error codes to SDK error variants. | Low |

### 3.4 `src/keypair.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | No `from_file()` / `save_to_file()` — Python SDK has `load()` / `save()`. | Medium |
| 2 | Missing functionality | No `from_bytes()` for 64-byte secret key. | Low |
| 3 | Security | `to_seed()` returns `[u8; 32]` — seed is in-memory. No zeroization on drop. Should use `zeroize` crate. | Medium |

### 3.5 `src/transaction.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Serialization | Relies on `moltchain_core::Message::serialize()`. If core changes field order, all SDKs must update. | Medium |
| 2 | Missing functionality | No multi-signer support. | Medium |
| 3 | API consistency | No static helper methods for common instructions (`transfer`, `stake`, `unstake`) — JS and Python SDKs have these. | Medium |

### 3.6 `src/types.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Positive | `Balance::from_molt()` gracefully handles NaN, negative, overflow. Well-tested. | Info |
| 2 | Type safety | `Block` uses `transaction_count: u64` but JS SDK's `Block` type uses `transactions: number`. Field name mismatch. | Low |
| 3 | Missing functionality | No `Validator`, `ChainStatus`, `Metrics` types — these remain untyped `Value`. | Medium |

### 3.7 `Cargo.toml`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | No websocket dependency — WebSocket support absent. | High |
| 2 | Positive | Dependencies are modern (`base64 = "0.22"`, `thiserror = "2.0"`). | Info |

---

## 4. DEX SDK (`dex/sdk/`)

### 4.1 `src/index.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | API consistency | `VERSION = '1.0.0'` while main SDK is `0.1.0`. Version mismatch implies the DEX SDK is "stable" while the core SDK is not. | Low |
| 2 | Positive | Clean barrel exports with all types. | Info |

### 4.2 `src/client.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | `wallet: any` in `MoltDEXConfig` — no type safety on wallet object. Any object with a `pubkey` property is accepted without validation. | Medium |
| 2 | Security | API key is sent as `X-API-Key` header — over HTTP (default endpoint is `http://localhost:8899`), the API key is transmitted in cleartext. | High |
| 3 | Security | API key is also appended to WebSocket URL as query parameter (`?api_key=...`) — visible in server logs, browser history, and referrer headers. | High |
| 4 | API consistency | `removeLiquidity()` sends DELETE to `/api/v1/pools/positions/${positionId}` — the `params` object is sent as request body, but HTTP DELETE body is undefined behavior per spec. Some servers/proxies strip it. | Medium |
| 5 | API consistency | `cancelOrder()` sends DELETE to `/api/v1/orders/${orderId}` — same issue with DELETE body. | Low |
| 6 | Security | `placeLimitOrder()` / `placeMarketOrder()` do not sign the order — no wallet signature is attached. The server authenticates via API key or session, not via cryptographic proof. | High |
| 7 | Security | No transaction signing for any operation — all operations go through REST API. This is a **centralized custody model**, not on-chain. | High |
| 8 | Type safety | `wallet: any` in config — should be `Keypair` from `@moltchain/sdk`. | Medium |
| 9 | Missing functionality | No rate limiting on client side. | Low |
| 10 | Positive | `request()` has `AbortController` with configurable timeout. | Info |
| 11 | Positive | `rpc()` has proper cleanup with `finally`. | Info |

### 4.3 `src/amm.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Serialization | `decodePool()` reads pool from raw bytes with hardcoded offsets. No magic bytes, no version field. If the on-chain struct layout changes, this silently produces garbage. | High |
| 2 | Serialization | `estimateSwapOutput()` uses simplified constant-product formula, but the DEX describes concentrated liquidity (tick-based). **The estimate will be wrong for any real pool.** | High |
| 3 | Serialization | `priceToSqrtPrice()` uses `Math.round(Math.sqrt(price) * 2^32)` — floating point precision loss for extreme prices. `BigInt` math should be used. | Medium |
| 4 | Missing functionality | No buffer length validation in `decodePool()` for short buffers. Out-of-bounds reads on malformed data. | Medium |
| 5 | Positive | Encode functions (`encodeCreatePool`, `encodeAddLiquidity`, etc.) have documented byte layouts. | Info |

### 4.4 `src/margin.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Type safety | `isLiquidatable()` converts `BigInt(Number(margin))` — lossy conversion through `Number` for large values. Should stay in `BigInt`. | High |
| 2 | Serialization | `PNL_BIAS = BigInt('9223372036854775808')` (2^63) for biased PnL encoding. Non-standard; if the contract changes the bias convention, all clients silently break. | Medium |
| 3 | Missing functionality | No buffer length validation in `decodeMarginPosition()`. | Medium |
| 4 | Positive | `liquidationPrice()`, `unrealizedPnl()`, `marginRatio()`, `effectiveLeverage()` well-implemented with proper BigInt arithmetic. | Info |

### 4.5 `src/orderbook.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | No buffer length validation in `decodeOrder()`. | Medium |
| 2 | Stub | `buildOrderBook()` hardcodes `orders: 1` for every level — doesn't reflect the actual number of orders at that price level. | Medium |
| 3 | Positive | `midPrice()` and `spreadBps()` handle empty books gracefully. | Info |

### 4.6 `src/router.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | `decodeSwapRecord()` — first 32 bytes (trader address) are not decoded. Data loss. | Medium |
| 2 | Error handling | `suggestRouteType()` returns `'clob'` when both CLOB and AMM have zero liquidity — will fail silently. Should throw or return `null`. | Medium |
| 3 | Type safety | `calculateMinOutput()` uses `Math.floor()` — for very small outputs this rounds to 0, meaning "accept any output." Allows sandwich attacks. | Medium |

### 4.7 `src/types.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Positive | Comprehensive type definitions covering all DEX domains. | Info |
| 2 | Type safety | `MoltDEXConfig.wallet` typed as `any`. | Medium |
| 3 | Type safety | `Trade.price` and `Trade.quantity` are `number` but `Order.price` is `bigint` — inconsistency within same domain. | Medium |
| 4 | Type safety | `Stats24h` — `volume` field is `number`, could exceed `Number.MAX_SAFE_INTEGER`. | Low |

### 4.8 `src/websocket.ts`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Positive | **Auto-reconnect** with exponential backoff (`1s → 30s max`). Much better than main JS SDK. | Info |
| 2 | Positive | Re-subscribes all active channels on reconnect. | Info |
| 3 | Positive | `close()` properly cleans up: clears timer, subscriptions, pending, sets `onclose = null`. | Info |
| 4 | Security | API key in WebSocket URL query parameter — visible in logs. | Medium |
| 5 | WebSocket | No heartbeat/ping mechanism. | Medium |
| 6 | Missing functionality | Uses `require('ws')` for Node.js — dynamic `require` won't work with ESM bundles. | Low |
| 7 | Error handling | `onmessage` swallows JSON parse errors silently. | Low |

### 4.9 `package.json` & `tsconfig.json`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Missing functionality | No `jest` in devDependencies despite `scripts.test: jest`. | Medium |
| 2 | Positive | `@moltchain/sdk` is `peerDependency` (optional). Good. | Info |

---

## 5. Main Contract SDK (`sdk/src/`)

### 5.1 `lib.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Positive | `#![no_std]` — proper for WASM smart contracts. Uses `dlmalloc` allocator. | Info |
| 2 | Positive | Panic handler uses `core::arch::wasm32::unreachable()` instead of `loop {}`. | Info |
| 3 | Positive | `test_mock` module with thread-local storage for unit testing without WASM runtime. | Info |
| 4 | Positive | `storage_read` buffer 65536 bytes (noted fix T5.14). | Info |
| 5 | Positive | `log::info` uses `core::hint::black_box` to prevent LTO from dead-code-eliminating the log import. | Info |
| 6 | Missing functionality | `storage::remove` is implemented as `set(key, &[])` — writes empty value rather than truly deleting. If the runtime treats empty as "exists," subtle bugs result. | Low |
| 7 | Missing functionality | No `storage::exists()` — must use `get().is_some()`. An empty value (from `remove()`) would count as existing. | Low |

### 5.2 `crosscall.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Stub | Non-WASM mock `call_contract()` always returns `Ok(Vec::new())`. Cross-contract calls in tests always succeed with empty data — masks bugs. | Medium |
| 2 | Security | No reentrancy guard — contracts can call each other recursively without limit (depends entirely on runtime enforcement). | Medium |
| 3 | Missing functionality | `CrossCall` struct exists but has no `execute()` method — callers must use free function `call_contract()`. OO API incomplete. | Low |
| 4 | Missing functionality | `cross_contract_call` return buffer is 65536 bytes. Larger responses are silently truncated. | Low |

### 5.3 `dex.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | API consistency | On-chain AMM is constant-product (x*y=k) with 0.3% fee. DEX SDK TypeScript describes concentrated liquidity with ticks and sqrt prices. **Fundamental architecture mismatch between on-chain and off-chain.** | Critical |
| 2 | Missing functionality | `fee_numerator` / `fee_denominator` are NOT persisted — `load()` reconstructs with defaults (`3/1000`). If governance changes fees, they revert on next `load()`. | High |
| 3 | Missing functionality | `save()` / `load()` store pool state as separate keys. **Non-atomic** — if process crashes mid-save, partial state is persisted. | Medium |
| 4 | Missing functionality | No slippage protection in `add_liquidity` — if pool price has moved, users can be front-run. | Medium |
| 5 | Missing functionality | No flash loan protection. | Low |
| 6 | Missing functionality | No protocol fee accrual. | Low |
| 7 | Positive | Uses u128 intermediate arithmetic to prevent overflow. Well-tested. | Info |
| 8 | Positive | `sqrt()` implementation handles large u128 values correctly. | Info |

### 5.4 `nft.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | **`mint()` does not check caller authorization** — anyone can call `mint()` on any NFT contract. The `minter` is stored in `initialize()` but never verified in `mint()`. | Critical |
| 2 | Security | `burn()` sets owner to `&[]` (empty bytes) rather than deleting. `exists()` checks `storage_get(key).is_some()` — empty value is `Some(vec![])`, so `exists()` returns `true` for burned tokens. **Burned tokens appear to still exist.** | High |
| 3 | Missing functionality | `burn()` does not decrement `total_minted` — supply tracking becomes incorrect after burns. | Medium |
| 4 | Missing functionality | No `safeTransferFrom` (check if recipient can receive NFTs). | Low |
| 5 | Missing functionality | No royalty support (no EIP-2981 equivalent). | Low |
| 6 | Positive | Full MT-721 implementation: mint, transfer, burn, approve, setApprovalForAll, transferFrom. | Info |

### 5.5 `token.rs`

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | **`burn()` does not check caller authorization** — anyone can burn any address's tokens. `burn(victim_address, amount)` subtracts from the victim's balance with no permission check. | Critical |
| 2 | Missing functionality | Integer underflow: `burn()` computes `current_supply - amount` without `checked_sub`. In release mode (Rust u64 wrapping), if `current_supply < amount`, this wraps around. | Medium |
| 3 | Missing functionality | No `Transfer` event emission — no way to index transfers off-chain. | Medium |
| 4 | Missing functionality | No `Approval` event emission. | Low |
| 5 | Missing functionality | `decimals` is stored in struct but never written to / read from storage. Lost on reload. | Low |
| 6 | Positive | `mint()` properly checks `caller != owner` (i.e., only owner can mint). | Info |
| 7 | Positive | Reads `total_supply` from storage each time (no stale struct field). | Info |

---

## 6. Shared Wallet-Connect (`shared/wallet-connect.js`)

| # | Category | Finding | Severity |
|---|----------|---------|----------|
| 1 | Security | Creates wallet via `createWallet` RPC — **server-side key generation**. The server holds the private key. This is a custodial model. | High |
| 2 | Security | Last-resort fallback generates a random 44-character "address" from `crypto.getRandomValues` that is **not a valid Ed25519 public key** — random characters from a base58 alphabet. Operations with this address will fail. | High |
| 3 | Security | Wallet data stored in `localStorage` — accessible to any JavaScript on the same origin. XSS attack = wallet compromise. | Medium |
| 4 | Security | No HTTPS enforcement — wallet connects over plain HTTP by default. | Medium |
| 5 | Missing functionality | No transaction signing capability — the wallet only stores an address, not keys. All operations require server-side signing. | High |
| 6 | API consistency | `refreshBalance()` reads `result.balance || result.value || 0` — doesn't match any SDK's balance response format. JS SDK returns `{ shells, molt }`. | Medium |
| 7 | API consistency | Default `rpcUrl` is `http://localhost:9000` but all SDKs default to port `8899`. Port mismatch. | Medium |
| 8 | Missing functionality | No `disconnect()` cleanup for the balance polling interval. If the page navigates without calling `disconnect()`, interval continues firing errors. | Low |
| 9 | Missing functionality | No TypeScript types. Global `window` scope only, no module system. | Low |

---

## 7. Cross-SDK Consistency

### 7.1 Feature Matrix

| Feature | JS SDK | Python SDK | Rust SDK | DEX SDK |
|---------|--------|------------|----------|---------|
| Keypair generate | ✅ | ✅ | ✅ | N/A |
| Keypair save/load | ❌ | ✅ | ❌ | N/A |
| Transfer instruction helper | ✅ | ✅ | ❌ | N/A |
| Stake/Unstake helpers | ✅ | ✅ | ✅ | N/A |
| WebSocket subscriptions | ✅ (10 types) | ✅ (10 types) | ❌ | ✅ (6 types) |
| Auto-reconnect | ❌ | ❌ | ❌ | ✅ |
| simulateTransaction | ✅ | ❌ | ❌ | N/A |
| getProgramAccounts | ✅ | ❌ | ❌ | N/A |
| getContractAbi | ✅ | ❌ | ❌ | N/A |
| NFT queries | ✅ (5 methods) | ✅ (5 methods) | ✅ (5 methods) | N/A |
| Retry logic | ❌ | ❌ | ❌ | ❌ |
| Connection pooling | ❌ | ✅ (httpx) | ✅ (reqwest) | ❌ |
| Resource cleanup | ❌ | ✅ (async with) | ❌ | ✅ (disconnect) |
| Typed responses | Partial | ❌ (all Dict) | Partial | ✅ |
| Error wrapping | Basic | Basic | ✅ (Error enum) | Basic |
| Multi-signer | ❌ | ❌ | ❌ | N/A |

### 7.2 Data Format Inconsistencies

| Issue | Details | Severity |
|-------|---------|----------|
| Blockhash encoding | JS and Python `bincode` assume hex blockhash. Rust delegates to `Hash::from_hex()`. If RPC returns base58, JS/Python encoding breaks. | High |
| Balance response | JS expects `{ shells, molt, ... }`. Python returns raw dict. Rust reads only `shells`. `wallet-connect.js` expects `{ balance }` or `{ value }`. Four incompatible interpretations. | Medium |
| Transaction encoding | JS/Python use manual bincode byte layout. Rust uses `bincode::serialize()` on serde-derived struct. If field order differs, **cross-SDK transactions are incompatible**. | High |
| Block type | Rust `Block.transaction_count: u64` vs JS `Block.transactions: number`. Field name mismatch. | Low |
| Validator type | JS/Python return untyped JSON. Rust returns `Vec<Value>`. No SDK has a `Validator` struct. | Low |

### 7.3 RPC Methods Not in Any SDK

These methods are used in test scripts but are not exposed in any SDK's `Connection` class:

- `getRecentTransactions`, `getFeeConfig`, `getRentParams`
- `getAccountTxCount`, `getTransactionsByAddress`
- `getTokenAccounts`, `getAllSymbolRegistry`, `getSymbolRegistry`
- `getReefStakePoolInfo`, `getUnstakingQueue`, `getRewardAdjustmentInfo`
- `getStakingPosition`, `getContractEvents`
- `createWallet`, `requestAirdrop`
- `callContract`, `deployContract`
- `mintToken`, `setContractStorage`, `updateAccount`

---

## 8. Summary Matrix

### Critical Issues (Must Fix Before Production)

| # | Location | Issue |
|---|----------|-------|
| C1 | JS `keypair.ts` | Secret key publicly accessible via `keypair.secretKey` |
| C2 | JS `connection.ts` | No WebSocket reconnect — subscriptions permanently lost on disconnect |
| C3 | Python `connection.py` | No WebSocket reconnect — same issue |
| C4 | Contract `nft.rs` | `mint()` does not check caller authorization — anyone can mint |
| C5 | Contract `token.rs` | `burn()` does not check caller authorization — anyone can burn anyone's tokens |
| C6 | Contract `dex.rs` vs DEX SDK | On-chain AMM is x*y=k; DEX SDK types/encoding assume concentrated liquidity with ticks. Fundamental architecture mismatch. |
| C7 | Contract `nft.rs` | `burn()` sets owner to empty bytes but `exists()` still returns `true` for burned tokens |

### High Issues

| # | Location | Issue |
|---|----------|-------|
| H1 | JS/Python `bincode` | Blockhash assumed hex — breaks if RPC returns base58 |
| H2 | JS `transaction.ts` | `amount: number` loses precision above 2^53 (~9 MOLT) |
| H3 | JS `connection.ts` | No WebSocket heartbeat/ping |
| H4 | JS `connection.ts` | No WebSocket `close` handler |
| H5 | Python `connection.py` | No WebSocket heartbeat/ping |
| H6 | Python `keypair.py` | `save()` writes plaintext seed to disk, no encryption, no file permissions |
| H7 | Rust SDK | No WebSocket support at all |
| H8 | DEX `client.ts` | API key transmitted in cleartext over HTTP |
| H9 | DEX `client.ts` | API key in WebSocket URL query parameter — visible in logs |
| H10 | DEX `client.ts` | No transaction signing — centralized custody model |
| H11 | DEX `amm.ts` | `estimateSwapOutput` uses wrong formula for concentrated liquidity |
| H12 | DEX `margin.ts` | `isLiquidatable()` has lossy BigInt→Number→BigInt conversion |
| H13 | Contract `dex.rs` | Pool fee params not persisted — revert to defaults on reload |
| H14 | `wallet-connect.js` | Server-side key generation (custodial) + invalid fallback addresses |
| H15 | Cross-SDK | Transaction encoding: manual bincode (JS/Python) vs serde bincode (Rust) may diverge |
| H16 | DEX `amm.ts` | No buffer length validation — crashes on malformed data |
| H17 | Contract `nft.rs` | Burned tokens have owner set to empty but `exists()` returns true |
| H18 | `wallet-connect.js` | No signing capability — address-only wallet |

### Counts

| Severity | Count |
|----------|-------|
| Critical | 7 |
| High | 18 |
| Medium | ~40 |
| Low | ~35 |
| Info | ~20 |

**Total distinct findings: ~120**

---

*End of Audit*
