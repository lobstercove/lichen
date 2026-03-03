# MoltChain SDK & Frontend — Comprehensive File-by-File Audit

**Scope:** All SDK files (JS, Python, Rust, DEX, Core) and all frontend files (DEX, Wallet, Explorer, Faucet, Shared).  
**Method:** Full source read of every file, line by line.  
**Severity:** `[CRITICAL]` exploitable / data loss, `[HIGH]` functional / security risk, `[MEDIUM]` correctness / maintainability, `[LOW]` style / minor.

---

## Table of Contents

1. [JS SDK (sdk/js/src/)](#1-js-sdk)
2. [Python SDK (sdk/python/moltchain/)](#2-python-sdk)
3. [Rust SDK (sdk/rust/src/)](#3-rust-sdk)
4. [DEX SDK (dex/sdk/src/)](#4-dex-sdk)
5. [Core SDK (sdk/src/)](#5-core-sdk)
6. [Frontend — dex/dex.js](#6-frontend-dexjs)
7. [Frontend — wallet/js/wallet.js](#7-frontend-walletjs)
8. [Frontend — wallet/js/identity.js](#8-frontend-identityjs)
9. [Frontend — wallet/js/crypto.js](#9-frontend-cryptojs)
10. [Frontend — explorer/js/explorer.js](#10-frontend-explorerjs)
11. [Frontend — faucet/faucet.js](#11-frontend-faucetjs)
12. [Frontend — shared/wallet-connect.js](#12-frontend-wallet-connectjs)
13. [Frontend — shared-config.js](#13-frontend-shared-configjs)
14. [Cross-Cutting / Cross-SDK Issues](#14-cross-cutting)

---

## 1. JS SDK (`sdk/js/src/`)

### `index.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-1 | `[LOW]` | ~1-30 | Barrel re-export file. No issues. |

### `connection.ts` (755 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-2 | `[CRITICAL]` | ~377 | `setContractAbi(programId, abi)` — Sends an unauthenticated RPC call that sets the ABI for any contract on the node. Any caller can overwrite contract ABIs on any public RPC endpoint. Must require admin auth or be removed from the public SDK. |
| JS-3 | `[HIGH]` | ~45-50 | `nextId` counter is shared between HTTP RPC and WebSocket calls. If both are active concurrently, response routing can collide when the same ID is in-flight on both channels. |
| JS-4 | `[HIGH]` | ~620-660 | WebSocket `subscribe()` sets a 5-second timeout but has no reconnect logic. If the WS disconnects mid-subscription, the subscription silently dies. No heartbeat/ping. |
| JS-5 | `[HIGH]` | ~680-710 | `subscribeSignature()` resolves on first WS message with matching subscription but never validates the confirmation status string (e.g., `"confirmed"` vs `"finalized"`). |
| JS-6 | `[MEDIUM]` | ~90-100 | `rpcCall()` catches fetch errors but re-throws generic `Error(data.error.message)` without preserving the original error code. Consumers lose RPC error code context. |
| JS-7 | `[MEDIUM]` | ~410 | `getTokenAccounts(owner)` returns `result.accounts ?? []` — if the RPC returns an unexpected shape (e.g., `result.token_accounts`), this silently returns `[]` with no warning. |
| JS-8 | `[MEDIUM]` | ~430-460 | `sendTransaction()` serialises the transaction to JSON, base64-encodes it, and sends via RPC. The JSON serialization format differs from the Rust SDK's bincode format — a transaction built by the JS SDK cannot be verified by the Rust SDK and vice versa. |
| JS-9 | `[LOW]` | ~500-530 | `getRecentBlockhash()` caches for 30 seconds. If the cluster has fast block times, stale blockhashes could cause transaction rejection. |
| JS-10 | `[LOW]` | ~300-330 | Several RPC methods (`getStakeAccount`, `getValidatorInfo`, etc.) have near-identical boilerplate. Could be DRY'd with a generic caller. |

### `keypair.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-11 | `[MEDIUM]` | ~25-30 | `Keypair.generate()` uses `tweetnacl.sign.keyPair()` which relies on `crypto.getRandomValues`. Safe in browsers and Node ≥15, but in older Node the polyfill may not be cryptographically strong. No explicit entropy check. |
| JS-12 | `[LOW]` | ~40 | `Keypair.fromSecretKey(key)` accepts a 64-byte Uint8Array but does not validate that bytes 32-63 match the Ed25519 public key derived from bytes 0-31. Corrupt keys pass silently. |

### `publickey.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-13 | `[LOW]` | ~15 | `PublicKey.equals()` does byte-by-byte comparison. Correct, but no constant-time comparison — not a security issue for public keys but inconsistent with crypto best practice. |

### `transaction.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-14 | `[CRITICAL]` | ~30-40 | `amount` field is typed as `number` (JavaScript IEEE 754 float). Values above `Number.MAX_SAFE_INTEGER` (2^53 ≈ 9 × 10^15) lose precision. Since 1 MOLT = 1e9 shells, amounts above ~9,007,199 MOLT will be corrupted. Must use `bigint` or string. |
| JS-15 | `[HIGH]` | ~55-70 | `Transaction.sign(keypair)` signs the JSON-serialized message. The serialization order is not canonicalized — if `JSON.stringify` key ordering changes between environments (e.g., V8 vs. SpiderMonkey for integer keys), signatures will be non-portable. |
| JS-16 | `[MEDIUM]` | ~80-90 | `serialize()` returns a JSON string of the entire transaction including signature. This is not bincode format — differs from what the validator expects if it uses Rust's `bincode::deserialize`. The connection layer re-wraps this in base64, adding an extra encoding layer. |

### `bincode.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| JS-17 | `[MEDIUM]` | ~1-120 | Implements a subset of bincode: u8, u16, u32, u64 (as bigint), i64, bool, string (length-prefixed), bytes, pubkey (32 bytes). Missing: i8, i16, i32, f32, f64, Option<T>, enum variants, struct alignment. If any on-chain struct uses these, serialization will fail silently. |
| JS-18 | `[LOW]` | ~45-55 | `encodeU64(value)` accepts `bigint | number`. The `number` path uses `Math.floor()` which can lose precision for values > 2^53 — same issue as JS-14 but in the bincode layer. |

---

## 2. Python SDK (`sdk/python/moltchain/`)

### `__init__.py`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-1 | `[LOW]` | ~1-10 | Re-exports. No issues. |

### `connection.py` (520 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-2 | `[HIGH]` | — | Missing RPC methods vs JS SDK: `getProgramAccounts`, `simulateTransaction`, `getContractAbi`, `setContractAbi`, `getTokenSupply`, `getEpochInfo`. Python SDK is incomplete — consumers cannot perform program account scanning or simulation. |
| PY-3 | `[HIGH]` | ~350-380 | `subscribe_account()` / `subscribe_signature()` use `asyncio.wait_for(timeout=10)` but have no reconnection or re-subscription logic. If the WebSocket drops, subscriptions silently fail. |
| PY-4 | `[MEDIUM]` | ~60-80 | `send_transaction()` serialises via `json.dumps()` + base64. Same JSON vs bincode mismatch as JS SDK (JS-16). |
| PY-5 | `[MEDIUM]` | ~100 | `get_balance()` returns raw integer (shells). JS SDK returns the same, but the Python SDK docstring says "Returns balance in MOLT" — misleading documentation. |
| PY-6 | `[LOW]` | ~420-440 | WebSocket `_ws_connect()` hardcodes `ws://` scheme. No TLS (`wss://`) support. Production deployments over the internet are unencrypted. |

### `keypair.py`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-7 | `[MEDIUM]` | ~20-25 | `Keypair.generate()` uses `nacl.signing.SigningKey(os.urandom(32))`. `os.urandom` is CS-PRNG on all platforms. Correct. However, `from_secret_key()` accepts 64-byte key without validating public key portion (same as JS-12). |

### `publickey.py`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-8 | `[LOW]` | ~30 | `PublicKey.__eq__` compares `self.bytes == other.bytes`. For `bytes` type in Python this is content comparison. Correct. |

### `transaction.py`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-9 | `[HIGH]` | ~35-40 | `amount` parameter is Python `int` (arbitrary precision), so no precision loss like JS. However, no upper bound validation — a user could pass `amount=2**256` and it would be serialized as a massive JSON integer that the validator cannot parse. |
| PY-10 | `[MEDIUM]` | ~60-70 | `sign()` signs `json.dumps(message).encode()`. Python's `json.dumps` sorts keys differently than JS `JSON.stringify` — cross-SDK signature verification will fail for the same logical transaction. |

### `bincode.py`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| PY-11 | `[MEDIUM]` | ~1-100 | Same subset as JS bincode. Missing types align with JS-17. No `Option<T>` or enum support. |
| PY-12 | `[LOW]` | ~50 | `encode_string(s)` uses `s.encode('utf-8')`. Correct. But no maximum length check — could create an oversized buffer if `s` is very long. |

---

## 3. Rust SDK (`sdk/rust/src/`)

### `lib.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-1 | `[LOW]` | ~1-20 | Module declarations only. No issues. |

### `client.rs` (500+ lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-2 | `[HIGH]` | — | **No WebSocket support at all.** The Rust SDK cannot subscribe to account changes, new blocks, or signature confirmations. Feature parity gap with JS and Python SDKs. |
| RS-3 | `[HIGH]` | ~80-100 | `send_transaction()` uses `bincode::serialize()` for the transaction body and sends it as a hex string. This is a different wire format than JS/Python (which use JSON + base64). A transaction built with the Rust SDK cannot be sent through the JS SDK's connection and vice versa. |
| RS-4 | `[MEDIUM]` | ~150-170 | `get_balance()` returns `u64`. Correct for shell amounts, but max value is 18.4 × 10^18 shells = 18.4 billion MOLT. This is sufficient for any realistic supply but should be documented. |
| RS-5 | `[MEDIUM]` | ~200-220 | Error handling uses `reqwest::Error` wrapped in a custom `SdkError`. HTTP status codes are not differentiated — a 429 (rate limit) is treated the same as a 500 (server error). |
| RS-6 | `[LOW]` | ~300-350 | Several methods clone strings unnecessarily (`address.to_string()` when `&str` would suffice). Minor efficiency issue. |

### `keypair.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-7 | `[LOW]` | ~40-50 | `Keypair::from_bytes()` validates that the 64-byte input produces a valid Ed25519 keypair via `ed25519_dalek::Keypair::from_bytes()`. This is better than the JS/Python SDKs which skip this check. |

### `transaction.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-8 | `[MEDIUM]` | ~20-35 | `Transaction::sign()` signs `bincode::serialize(&self.message)`. Since `serde` + `bincode` use a deterministic encoding for the same struct, this is reproducible. However, the struct field order must never change, or signatures break. Fields should have `#[serde(rename)]` or explicit order attributes. |
| RS-9 | `[LOW]` | ~50-60 | `amount` is `u64`. Correct. No precision issues. |

### `types.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-10 | `[LOW]` | ~1-70 | Type definitions for `AccountInfo`, `BlockInfo`, `TransactionInfo`, etc. Clean. No issues. |

### `error.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| RS-11 | `[LOW]` | ~1-40 | `SdkError` enum with `RpcError`, `SerializationError`, `SigningError`, `NetworkError`. Adequate. |

---

## 4. DEX SDK (`dex/sdk/src/`)

### `index.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-1 | `[LOW]` | ~1-15 | Barrel re-export. No issues. |

### `client.ts` (509 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-2 | `[HIGH]` | ~73 | `wallet` property is typed as `any`. No type safety for the wallet integration — any object is accepted. Could pass a wallet missing `signTransaction()` and get a runtime crash deep in the call stack. |
| DX-3 | `[MEDIUM]` | ~120-140 | `placeLimitOrder()` takes `price: number` and multiplies by `PRICE_SCALE` (1e9). Floating-point multiplication can produce rounding errors (e.g., `1.1 * 1e9 = 1099999999.9999998`). Should use integer arithmetic or `Math.round()`. |
| DX-4 | `[MEDIUM]` | ~180-200 | `placeMarketOrder()` hardcodes `price: 0` in the instruction. The contract must interpret `price=0` as "market order" — if it doesn't, the order will be a limit at price 0 (free tokens). |
| DX-5 | `[MEDIUM]` | ~300-320 | `getOrderBook()` fetches from REST API `/api/v1/orderbook/{pairId}`. No pagination — if the order book has thousands of levels, the entire response is loaded into memory. |
| DX-6 | `[LOW]` | ~400-450 | `getTradingPairs()` caches result for 60 seconds. Adequate for typical use. |

### `amm.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-7 | `[MEDIUM]` | ~50-60 | `addLiquidity()` takes `tickLower` and `tickUpper` as raw integers with no validation that they fall within `[MIN_TICK, MAX_TICK]` (-887272 to 887272). Out-of-range ticks would produce a contract error, but a client-side guard would give a better UX. |
| DX-8 | `[MEDIUM]` | ~80-90 | `removeLiquidity()` takes `positionId` as a number. No check for `positionId <= 0` or non-integer values. |
| DX-9 | `[LOW]` | ~100-120 | `getPoolStats()` returns pool TVL, volume, and fee APR. No caching — each call is a fresh RPC request. |

### `margin.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-10 | `[MEDIUM]` | ~40-60 | `openPosition()` takes `leverage` as a number. No validation that it matches the contract's allowed tiers (2x, 3x, 5x, 10x, 20x). Passing `leverage=100` would be sent to the contract, which may reject it or (worse) accept it. |
| DX-11 | `[MEDIUM]` | ~100-120 | `addMargin()` and `removeMargin()` do not check that the amount is positive. Negative amounts could have undefined behavior. |
| DX-12 | `[LOW]` | ~140-160 | `getPositions()` returns all positions for a user. No limit/pagination parameter. |

### `orderbook.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-13 | `[HIGH]` | ~30-40 | `submitOrder()` computes `Math.round(params.price * PRICE_SCALE)` where `PRICE_SCALE = 1e9`. For prices with many decimal places (e.g., `0.000000001`), the multiplication can produce float errors. `BigInt` or a decimal library should be used. |
| DX-14 | `[MEDIUM]` | ~55-70 | `modifyOrder()` takes `newPrice` and `newQuantity` but does not validate that at least one is provided. Sending both as `undefined` would submit a no-op modification. |

### `router.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-15 | `[MEDIUM]` | ~20-40 | `getQuote()` calls the REST API `/api/v1/router/quote`. The response includes the optimal route, but the SDK does not validate that the route's `expectedOutput` is above a user-specified `minOutput`. Slippage protection is left entirely to the caller. |
| DX-16 | `[LOW]` | ~60-80 | `executeSwap()` accepts the route object from `getQuote()` and submits it. If the route is stale (fetched minutes ago), the swap could fail or execute at a worse price. No staleness check. |

### `types.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-17 | `[LOW]` | ~1-80 | Type definitions for `Order`, `Pool`, `Position`, `Route`, etc. Clean. |

### `websocket.ts`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DX-18 | `[HIGH]` | ~30-50 | WebSocket reconnection uses a fixed 5-second delay. No exponential backoff. In a disconnect storm, this hammers the server with reconnect attempts every 5 seconds. |
| DX-19 | `[MEDIUM]` | ~70-90 | `subscribe(channel, callback)` stores callbacks in a `Map<string, Function>`. If the same channel is subscribed twice, the first callback is silently replaced. No way to have multiple listeners per channel. |
| DX-20 | `[LOW]` | ~100-120 | `unsubscribe(channel)` sends an unsubscribe message to the server but does not wait for acknowledgment. The server may continue sending messages for that channel until it processes the unsub. |

---

## 5. Core SDK (`sdk/src/`) — Rust/WASM contract SDK

### `lib.rs` (447 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CORE-1 | `[HIGH]` | ~30-50 | `host_call(ptr, len)` is declared `extern "C"` and is the sole FFI bridge to the runtime. The return value convention is `(ptr << 32) | len` packed into a `u64`. If the host returns a length > 2^32, this silently truncates. No length validation in `unpack_result()`. |
| CORE-2 | `[HIGH]` | ~100-130 | `transfer(to, amount)` writes a JSON command `{"op":"transfer","to":"...","amount":N}` to the host. The amount is a `u64` serialized as a JSON number. JSON numbers in many parsers are limited to f64 precision (2^53), so amounts above ~9 quadrillion shells could be rounded by the host's JSON parser. Should use string representation. |
| CORE-3 | `[MEDIUM]` | ~200-230 | `emit_event(event_type, data)` serializes `data` as JSON string. No maximum size check — a contract could emit a multi-megabyte event, potentially DoS-ing the event storage. |
| CORE-4 | `[MEDIUM]` | ~250-270 | `get_account_info(address)` parses the host response as JSON. If the host returns malformed JSON, the contract panics with an opaque `unwrap()` error. Should return `Result`. |
| CORE-5 | `[LOW]` | ~400-447 | `log(msg)` writes to host. No log level support (debug/info/warn/error). |

### `crosscall.rs`

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CORE-6 | `[MEDIUM]` | ~20-40 | `cross_call(program_id, method, args)` builds a JSON payload with the method name and args. If `method` contains special characters (quotes, backslash), the JSON will be malformed. No escaping. |
| CORE-7 | `[LOW]` | ~50-60 | `cross_call_with_value(program_id, method, args, value)` includes `value` as a JSON number — same precision risk as CORE-2. |

### `dex.rs` (367 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CORE-8 | `[HIGH]` | ~40-60 | `place_order(pair_id, side, price, quantity)` encodes `price` and `quantity` as u64 into a fixed-size byte buffer using little-endian. The buffer layout is `[opcode(1), pair_id(8), side(1), order_type(1), price(8), quantity(8)] = 27 bytes`. If the contract ABI changes field order or adds fields, all SDK consumers break silently with corrupt data. No versioning. |
| CORE-9 | `[MEDIUM]` | ~100-120 | `add_liquidity()` uses the same byte-buffer approach. Tick values are encoded as `i32` LE. The function does not validate tick alignment to fee tier spacing. |
| CORE-10 | `[MEDIUM]` | ~200-230 | `open_margin_position()` encodes leverage as `u8`. Maximum leverage byte value is 255, but the contract likely only supports up to 20x. No client-side validation. |
| CORE-11 | `[LOW]` | ~300-367 | Helper functions for reading contract state (`get_order_book`, `get_pool_info`, etc.) parse JSON from host calls. Consistent pattern. |

### `token.rs` (MT-20)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CORE-12 | `[MEDIUM]` | ~30-50 | `transfer(to, amount)` does not check `amount > 0`. Zero-amount transfers would succeed as a no-op but consume gas. |
| CORE-13 | `[LOW]` | ~80-100 | `approve(spender, amount)` has the classic ERC-20 approve race condition. No `increaseAllowance/decreaseAllowance` functions. |

### `nft.rs` (MT-721)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CORE-14 | `[MEDIUM]` | ~20-40 | `mint(to, token_id, metadata_uri)` does not validate `metadata_uri` format or length. Arbitrarily long URIs could be stored on-chain. |
| CORE-15 | `[LOW]` | ~60-80 | `transfer_from(from, to, token_id)` checks ownership but the error message is a generic `panic!("not authorized")`. Should distinguish between "not owner" and "not approved". |

---

## 6. Frontend — `dex/dex.js` (5200 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| DEX-1 | `[CRITICAL]` | ~585-600 | `buildCreateMarketArgs()` — `question_hash` is computed with a simple XOR fold (`hash[i % 32] ^= byte`) of UTF-8 bytes. This is **not** a cryptographic hash — trivial to find collisions. Two different questions can produce the same 32-byte hash. If the contract uses this for uniqueness, duplicate markets can be created. |
| DEX-2 | `[CRITICAL]` | ~570-580 | `buildResolveMarketArgs()` — `attestation_hash` is hardcoded to 32 zero bytes (`new Uint8Array(32)`). Oracle verification of the resolution is completely skipped. Anyone who calls resolve can pass an empty attestation. |
| DEX-3 | `[HIGH]` | ~540-545 | `buildVoteArgs()` uses opcode `2`. `buildClaimRewardsArgs()` (line ~730) also uses opcode `2`. These target different contracts (dex_governance vs dex_rewards), but if a builder is accidentally called with the wrong contract address, the opcode collision means a vote could be misinterpreted as a reward claim or vice versa. |
| DEX-4 | `[HIGH]` | ~555-560 | `buildBuySharesArgs()` uses opcode `4`. `buildExecuteProposalArgs()` (line ~535) also uses opcode `4`. Same cross-contract opcode collision risk as DEX-3. |
| DEX-5 | `[HIGH]` | ~850-950 | `encodeTransactionMessage()` — The bincode serialization implementation must exactly match the Rust validator's `bincode::deserialize`. The code writes `recent_blockhash` as raw hex-decoded bytes (32 bytes), but if the RPC returns the blockhash as base58 instead of hex, the decoding will produce garbage. There is no format detection or validation. |
| DEX-6 | `[HIGH]` | ~630-640 | `buildChallengeResolutionArgs()` — `evidence_hash` uses the same weak XOR fold as `question_hash` when a string is provided. If `evidence` is a Uint8Array, it's used directly (which could be correct), but string evidence loses uniqueness through the XOR collision. |
| DEX-7 | `[HIGH]` | ~4500-4520 | `nextMarketId` for initial liquidity is guessed from `total_markets + 1`. This is a race condition — if two users create markets simultaneously, both compute the same `nextMarketId` and one's liquidity goes to the wrong market. The create+liquidity should be atomic or the market ID should come from the create receipt. |
| DEX-8 | `[MEDIUM]` | ~750-810 | ClawPump (Launchpad) builders (`buildCPCreateTokenArgs`, `buildCPBuyArgs`, `buildCPSellArgs`) use a **different ABI convention** than all other contract builders. They use named function ABI (JSON fields like `{"buyer":"...","token_id":1}`) instead of opcode-byte-buffer serialization. This inconsistency means the contract dispatch must handle two different serialization formats. |
| DEX-9 | `[MEDIUM]` | ~1350-1430 | TradingView `getBars()` custom datafeed — `onHistoryCallback` is called with bars, but if the API returns zero bars, `noData` is set to `true` and `onHistoryCallback([], {noData: true})` is called. If TradingView calls `getBars` again with the same range, it creates an infinite loop of no-data callbacks. |
| DEX-10 | `[MEDIUM]` | ~1400-1420 | `streamBarUpdate()` updates the last candle in the TradingView series. If a new candle period starts between polls (e.g., a new 1-minute candle), the function extends the previous candle instead of creating a new one. The `time` field is not checked against the current candle period. |
| DEX-11 | `[MEDIUM]` | ~2100-2150 | Binance WebSocket price feed (`connectBinancePriceFeed`) connects to `wss://stream.binance.com:9443/ws`. CORS and content security policies may block this in production. No CSP meta tag accommodation. |
| DEX-12 | `[MEDIUM]` | ~2200-2220 | Oracle price reference updates every 5 seconds via `setInterval(() => fetch('/api/v1/oracle/prices'))`. No `clearInterval` on view switch — continues fetching even when the trade view is not active. |
| DEX-13 | `[MEDIUM]` | ~2700-2760 | Margin tier parameters (`initialBps`, `maintenanceBps`, `liquidationPenaltyBps`) are hardcoded in the frontend. If the contract's tier params change, the frontend shows incorrect liquidation prices. These should be fetched from the contract. |
| DEX-14 | `[MEDIUM]` | ~3650-3680 | Delist proposal path — UI allows selecting delist type and filling in pair/reason, but submission is blocked with "not yet supported on-chain". The UI gives a false impression that this feature works. Should hide the UI option. |
| DEX-15 | `[MEDIUM]` | ~3685-3700 | Parameter change proposal path — same issue as DEX-14. UI lets user fill in parameter name/value but submission blocked. |
| DEX-16 | `[MEDIUM]` | ~4380-4400 | `updatePredictCalc()` CPMM formula uses `m.outcomes[outcomeIdx]?.pool_yes` for reserve values, but the API may not return `pool_yes` field. Fallback to linear pricing silently changes the trade estimate with no user indication that the shown price is approximate. |
| DEX-17 | `[LOW]` | ~580, 640 | Multiple `catch {}` empty catch blocks throughout the file (at least 15 instances). Errors are completely swallowed with no logging. Makes debugging production issues very difficult. |
| DEX-18 | `[LOW]` | ~960-970 | `MOLT_GENESIS_PRICE = $0.10` hardcoded. Used for USD conversion calculations. If the price changes, all USD values shown are incorrect until the hardcoded value is updated. |
| DEX-19 | `[LOW]` | ~1500-1510 | Order modification via `buildModifyOrderArgs` — the inline edit UI allows modifying price and quantity but does not re-run the preflight validation checks (tick alignment, lot size, min notional, etc.) that the initial order form performs. |
| DEX-20 | `[LOW]` | ~3060-3080 | Trade history CSV export — `toCSV()` does not escape commas or quotes in trade pair names. If a pair name contains a comma, the CSV will be malformed. |
| DEX-21 | `[LOW]` | ~3900-3950 | Prediction market card rendering builds large HTML strings via template literals. Multiple `escapeHtml()` calls are correct for XSS prevention, but `m.pm_id` and `m.cat` are used in `data-*` attributes without escaping, potentially allowing attribute injection if the API returns crafted values. |
| DEX-22 | `[LOW]` | ~4500-4510 | `currentSlot` fallback uses `Math.round(Date.now() / 400)` (400ms per slot). Comment at line 4510 says "F16.9: 400ms per slot" but line 3870 uses 0.5s (500ms) per slot for dispute countdown. Inconsistent slot time assumption. |
| DEX-23 | `[LOW]` | ~5130-5150 | Polling intervals: trade/pool/margin at 5s, governance/rewards/launchpad at 30s, prediction markets at 15s, pair prices at 10s. Four separate `setInterval` loops running concurrently. These do not coordinate — all four could fire at the same 5-second mark, causing a burst of API calls. |

---

## 7. Frontend — `wallet/js/wallet.js` (3787 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| W-1 | `[HIGH]` | ~430-470 | `serializeMessageBincode()` — Critical bincode serializer for signing. This implementation must bit-for-bit match the Rust validator's `bincode::deserialize`. The function writes `recent_blockhash` by hex-decoding the string from RPC. If the blockhash is base58-encoded (as some RPCs return), the deserialization produces incorrect bytes, and the signature will be invalid. No format detection. |
| W-2 | `[HIGH]` | ~2700-2730 | JSON keystore export (`exportKeystoreJSON`) includes the full 64-byte `secretKey` as a hex string in a downloaded `.json` file. This file is **not encrypted** — the private key is in plaintext. If the user's downloads folder is accessible, the key is exposed. The UI does warn about this, but the file itself provides no protection. |
| W-3 | `[HIGH]` | ~950-1000 | Wallet state loading from `localStorage` — `loadSavedWallets()` reads `moltchain_wallets` and parses JSON. The `AUDIT-FIX W-9` check validates `address` is a non-empty string, but does not validate it's a valid base58 address. Malformed addresses in localStorage could cause downstream RPC errors. |
| W-4 | `[MEDIUM]` | ~2500-2550 | `confirmSend()` — When sending MOLT (opcode 0), the amount is converted to shells via `Math.round(parseFloat(amount) * 1e9)`. `parseFloat` precision for large decimal numbers can lose significant digits. Same issue as JS-14. |
| W-5 | `[MEDIUM]` | ~2600-2620 | `confirmSend()` — Token transfer via contract Call instruction uses a JSON payload `{"op":"transfer","to":"...","amount":N}`. The `amount` is a number, not a string. If the token contract parses this with a JSON parser that uses f64, amounts > 2^53 are rounded. |
| W-6 | `[MEDIUM]` | ~1200-1250 | `refreshBalance()` uses `MOCK_PRICES` object with hardcoded prices (MOLT=$0.10, mUSD=$1.00, wSOL=$100, wETH=$2000, REEF=$0.01). These are never updated from a price feed. All USD values shown on the dashboard are based on stale mock prices. |
| W-7 | `[MEDIUM]` | ~1550-1600 | ReefStake modal — `stakeAmount` validation allows amounts down to `0.000000001 MOLT` (1 shell). On-chain minimum stake may be higher. No client-side minimum stake check. |
| W-8 | `[MEDIUM]` | ~2300-2350 | Bridge deposit flow — deposit address is fetched from `${CUSTODY_URL}/api/v1/deposit/address`. The response is displayed with XSS protection (`AUDIT-FIX W-C2`), but the deposit address is not validated against expected format (e.g., Solana base58 or Ethereum 0x hex). A compromised custody server could return a malicious address. |
| W-9 | `[MEDIUM]` | ~3100-3150 | Password modal system — `showPasswordModal()` returns a Promise that resolves on form submit. If the user dismisses via ESC or backdrop click, the Promise resolves with `null`. Some callers check for `null`, but others (e.g., `confirmSend` at ~2560) proceed without checking, which would crash on `decryptPrivateKey(null)`. |
| W-10 | `[MEDIUM]` | ~3300-3350 | Export private key — The key is briefly displayed in a textarea. There is a `AUDIT-FIX W-2` note about using DOM event listeners instead of inline onclick, but the key remains visible in the DOM until the modal is manually closed. No auto-clear timeout. |
| W-11 | `[MEDIUM]` | ~3500-3530 | Auto-lock timer — `AUDIT-FIX W-4` skips auto-lock when `timeout=0`. However, the default timeout is stored in localStorage. If `localStorage.getItem('autoLockTimeout')` returns `null` (first use), `parseInt(null)` returns `NaN`, and `NaN !== 0` is `true`, so auto-lock is enabled with `NaN` milliseconds — effectively immediate lock on any user interaction. Should default to a sensible value (e.g., 5 minutes). |
| W-12 | `[LOW]` | ~700-750 | Wallet creation — BIP39 mnemonic word list is hardcoded in the crypto.js file. If the word list is incomplete or modified, generated mnemonics may not be compatible with other wallets. |
| W-13 | `[LOW]` | ~1100-1120 | `loadAssets()` — Token list is loaded from a deploy manifest. If the manifest is unavailable, a hardcoded fallback list is used. The fallback list may become stale as new tokens are added. |
| W-14 | `[LOW]` | ~1400-1450 | `loadActivity()` — Transaction history merges airdrop transactions. The merge logic assumes airdrops are adjacent in the list. If the RPC returns them non-adjacently, they won't be merged. |
| W-15 | `[LOW]` | ~2900-2950 | `logoutWallet()` clears `localStorage` and `sessionStorage` but does not clear the in-memory `wallet.keypair` object. If `logoutWallet` is called but the page is not reloaded, the keypair remains in memory until GC collects it (non-deterministic). |
| W-16 | `[LOW]` | ~3600-3650 | Network selector — switching networks changes the RPC endpoint but does not disconnect/reconnect WebSocket subscriptions. The WS may continue pointing to the old network's endpoint. |

---

## 8. Frontend — `wallet/js/identity.js` (1193 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| ID-1 | `[MEDIUM]` | ~50-80 | WASM ABI layout encoding uses a `0xAB` magic prefix byte. If the contract changes its ABI prefix, all identity operations will fail with opaque errors. The prefix is hardcoded with no version negotiation. |
| ID-2 | `[MEDIUM]` | ~120-150 | `buildContractCall()` includes a pre-flight balance check (fetches balance, compares to fee). This adds an extra RPC round-trip before every identity operation. The balance could change between the check and the transaction submission (TOCTOU). |
| ID-3 | `[MEDIUM]` | ~1100-1150 | Enhanced `showPasswordModal()` wraps the original with a `setTimeout(50ms)` post-render to replace `<input type="text">` with `<select>` elements. This is a race condition — if the browser takes longer than 50ms to render the modal, the replacement fails silently and the user sees a text input instead of a dropdown. |
| ID-4 | `[MEDIUM]` | ~700-730 | `registerNameModal()` — Name pricing is computed client-side (`NAME_PRICING` constants). If the contract's pricing changes, the UI shows incorrect costs. Should validate against on-chain pricing. |
| ID-5 | `[LOW]` | ~200-230 | `renderProfileStrip()` escapes all server data with `escapeHtml()`. Correct. But `identityData.avatar_url` is used as an `<img src="">` — while `escapeHtml()` prevents XSS in the attribute value, a `javascript:` URI in newer browsers is blocked, but a data URI with SVG could potentially execute scripts in some contexts. Should validate protocol. |
| ID-6 | `[LOW]` | ~350-380 | `renderSkillsSection()` — Skill proficiency bars calculate width as `(skill.level / 100) * 100 + '%'`. If `skill.level` exceeds 100 (server-side bug), the bar overflows its container. Should clamp to 100. |
| ID-7 | `[LOW]` | ~500-530 | `renderAchievementsSection()` — Achievements are checked against `ACHIEVEMENT_DEFS` (hardcoded list of ~10 achievements). If the contract adds new achievement types, they won't render in the UI. |
| ID-8 | `[LOW]` | ~800-830 | `transferNameModal()` — `AUDIT-FIX W-6` validates the recipient address is a non-empty string, but does not check base58 format or length. An invalid address would be sent to the contract and rejected on-chain, wasting gas. |

---

## 9. Frontend — `wallet/js/crypto.js` (535 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| CR-1 | `[HIGH]` | ~100-140 | `encryptPrivateKey(key, password)` uses PBKDF2 with 100,000 iterations and SHA-256. This is below the OWASP 2023 recommendation of 600,000 iterations for PBKDF2-SHA256. With GPU acceleration, 100K iterations can be brute-forced at ~100K passwords/second. |
| CR-2 | `[MEDIUM]` | ~200-230 | `generateMnemonic()` uses the built-in BIP39 word list (2048 words). The entropy is 128 bits (12 words). 24-word (256-bit) option is not provided. For high-value wallets, 12 words may be insufficient. |
| CR-3 | `[MEDIUM]` | ~300-330 | `mnemonicToKeypair(mnemonic)` derives the keypair from the mnemonic using PBKDF2 with the mnemonic as password and `"moltchain"` as salt. This is **not** BIP39-standard (which uses `"mnemonic" + passphrase` as salt). Mnemonics are not compatible with standard BIP39 wallets (Phantom, MetaMask, etc.). |
| CR-4 | `[LOW]` | ~400-430 | AES-GCM encryption uses a 12-byte random IV. Correct. The IV is prepended to the ciphertext, which is standard. |
| CR-5 | `[LOW]` | ~460-490 | `decryptPrivateKey(encryptedHex, password)` — If decryption fails (wrong password), it throws a generic error. No distinction between "wrong password" and "corrupted data". |

---

## 10. Frontend — `explorer/js/explorer.js` (790 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| EX-1 | `[MEDIUM]` | ~30-80 | `MoltChainRPC` class is **duplicated** here (also in wallet.js and dex.js). Three independent implementations of the same RPC client. Any bug fix must be applied in three places. Should be extracted to a shared module. |
| EX-2 | `[MEDIUM]` | ~200-230 | Block explorer search — `search(query)` tries the query as a block number, transaction hash, and account address sequentially. If the query is a valid number that also happens to be a valid address prefix, it will match as a block number first. No disambiguation UI. |
| EX-3 | `[MEDIUM]` | ~400-430 | MoltName resolution — `resolveName(name)` calls `getMoltNameOwner(name)`. If the name doesn't resolve, it falls through to trying the raw query as an address. No error feedback for "name not found". |
| EX-4 | `[LOW]` | ~600-650 | Dashboard stats auto-refresh every 10 seconds. No `clearInterval` on page navigation — continues polling in background. |
| EX-5 | `[LOW]` | ~700-750 | Transaction detail page — `renderTransaction(tx)` displays instruction data as raw hex. No attempt to decode known opcodes into human-readable format. |

---

## 11. Frontend — `faucet/faucet.js` (160 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| FA-1 | `[MEDIUM]` | ~40-60 | Faucet form submits to `/api/v1/faucet/request` with the wallet address. The captcha token is sent but the server-side validation is not visible. If the server doesn't validate the captcha, the faucet is open to automated draining. |
| FA-2 | `[LOW]` | ~100-120 | Rate limiting message — "Please wait 60 seconds between requests" is hardcoded in the UI. If the server changes the rate limit, the message is inaccurate. |
| FA-3 | `[LOW]` | ~130-150 | Address input — no client-side validation of base58 format. Invalid addresses are sent to the server and rejected, but the error message from the server may be cryptic. |

---

## 12. Frontend — `shared/wallet-connect.js` (345 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| WC-1 | `[CRITICAL]` | ~280-310 | **Fallback address generation** — When the wallet extension is not available, `generateFallbackAddress()` creates a "fake" address by generating 32 random bytes and encoding them with a custom base58 function (`randomBytes.map(b => BASE58_CHARS[b % 58])`). This is **not** a valid Ed25519 public key — there is no corresponding private key. Any funds sent to this address are permanently locked. The function should either refuse to generate a fallback or clearly mark it as "view-only". |
| WC-2 | `[HIGH]` | ~150-180 | Cross-app wallet connection — `connectWallet()` attempts to read the keypair from `localStorage` in the wallet app's domain. Due to same-origin policy, this only works if the DEX and wallet are served from the same origin. If they're on different subdomains (e.g., `dex.moltchain.io` vs `wallet.moltchain.io`), the connection silently fails and falls back to WC-1's fake address. |
| WC-3 | `[MEDIUM]` | ~50-80 | `WalletConnect.disconnect()` removes the wallet from localStorage but does not notify other tabs. If the user has the DEX open in another tab, it continues showing the old wallet as connected. Should use `BroadcastChannel` or `storage` event. |
| WC-4 | `[LOW]` | ~200-230 | `isConnected()` checks `localStorage.getItem('moltchain_connected')`. This is a string `"true"` / `"false"` comparison. If the value is anything other than `"true"`, it returns false. Edge case: if another app writes a truthy non-`"true"` value, connection state is lost. |

---

## 13. Frontend — `shared-config.js` (43 lines)

| # | Severity | Line(s) | Finding |
|---|----------|---------|---------|
| SC-1 | `[LOW]` | ~1-43 | Network URL configuration. `RPC_URL`, `WS_URL`, `FAUCET_URL`, `EXPLORER_URL` are all pointing to `localhost`. No environment-based configuration (dev/staging/prod). In production, these must be updated manually. |
| SC-2 | `[LOW]` | ~20-30 | `CUSTODY_URL` is set to `http://localhost:3010`. No HTTPS. Production bridge deposits over HTTP expose transfer details and addresses. |

---

## 14. Cross-Cutting / Cross-SDK Issues

| # | Severity | Finding |
|---|----------|---------|
| X-1 | `[CRITICAL]` | **Wire format incompatibility** — JS and Python SDKs serialize transactions as JSON + base64. Rust SDK uses bincode + hex. The dex.js and wallet.js frontends use a custom bincode implementation. These three formats are mutually incompatible. A transaction signed by the JS SDK cannot be submitted through the Rust SDK's `send_transaction()` and vice versa. The validator must support all three formats or two of them silently fail. |
| X-2 | `[HIGH]` | **Signature non-portability** — JS SDK signs `JSON.stringify(message)`. Python SDK signs `json.dumps(message).encode()`. Rust SDK signs `bincode::serialize(&message)`. Even for the same logical transaction, the three SDKs produce different signatures because they sign different byte representations. A transaction cannot be signed by one SDK and verified by another. |
| X-3 | `[HIGH]` | **Missing features across SDKs** — Rust SDK has no WebSocket support. Python SDK is missing 6+ RPC methods available in JS SDK. DEX SDK has no Rust or Python bindings. This creates a first-class/second-class SDK hierarchy where JS is the only fully-featured SDK. |
| X-4 | `[HIGH]` | **Amount precision inconsistency** — JS SDK uses `number` (f64, loses precision at 2^53). Python SDK uses `int` (unlimited). Rust SDK uses `u64`. Core SDK uses `u64`. The same amount value can be represented differently across SDKs, leading to consensus failures. |
| X-5 | `[MEDIUM]` | **System program ID representation** — JS SDK uses base58 string `'11111111111111111111111111111111'`. Python SDK uses `b'\x00' * 32`. Rust SDK uses `Pubkey([0u8; 32])`. While logically equivalent, the different representations mean cross-SDK code comparison is error-prone. |
| X-6 | `[MEDIUM]` | **Duplicated code** — `MoltChainRPC` class is implemented independently in: `dex/dex.js`, `wallet/js/wallet.js`, `explorer/js/explorer.js`, `sdk/js/src/connection.ts`. Four implementations of the same client with slightly different features and bugs. |
| X-7 | `[MEDIUM]` | **Hardcoded prices** — `MOLT_GENESIS_PRICE = $0.10` in dex.js, `MOCK_PRICES = {MOLT: 0.10, wSOL: 100, wETH: 2000}` in wallet.js. No price oracle integration in frontends. All USD values shown are wrong if market prices deviate. |
| X-8 | `[LOW]` | **No SDK versioning** — None of the SDKs include a version number or protocol version in their requests. If the RPC API introduces breaking changes, older SDK versions fail with opaque errors. Should include a `X-SDK-Version` header or similar. |
| X-9 | `[LOW]` | **Inconsistent error handling** — JS SDK throws `Error(message)`, Python SDK raises custom exceptions, Rust SDK returns `Result<T, SdkError>`, DEX SDK throws `Error(message)`, Core SDK panics. No unified error taxonomy. |

---

## Summary Counts

| Severity | Count |
|----------|-------|
| **CRITICAL** | 6 |
| **HIGH** | 21 |
| **MEDIUM** | 41 |
| **LOW** | 33 |
| **Total** | **101** |

### Top Priority Fixes

1. **X-1 / X-2**: Unify transaction serialization format across all SDKs (pick one: bincode or JSON, not both)
2. **WC-1**: Remove or clearly mark fallback address generation — funds sent there are lost
3. **JS-14 / W-4 / X-4**: Use bigint for all amount parameters across JS SDK and frontends
4. **JS-2**: Remove `setContractAbi` from public SDK or gate behind auth
5. **DEX-2**: Implement real attestation hash in `buildResolveMarketArgs`
6. **DEX-1**: Replace XOR fold with SHA-256 or BLAKE3 for `question_hash`
7. **CR-1**: Increase PBKDF2 iterations to ≥600,000
8. **DEX-7**: Make market creation and initial liquidity atomic (return market ID from create)
9. **W-1 / DEX-5**: Add blockhash format detection (hex vs base58) in bincode serializers
10. **RS-2 / PY-2**: Implement WebSocket support in Rust SDK; add missing RPC methods to Python SDK
