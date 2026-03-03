# RPC & WebSocket Server — Production Readiness Audit

**Scope:** All `.rs` files in `rpc/src/` (7 files, ~21,000 lines) and `rpc/tests/` (4 files, ~4,500 lines)  
**Date:** 2026-02-25  
**Auditor:** Automated deep-read audit

---

## Executive Summary

The Moltchain RPC codebase is **remarkably mature**. Multiple prior audit rounds
have left extensive `AUDIT-FIX` annotations (F-1, F-2, F-5, F-13, 0.13, 2.15,
2.16, 3.25, A11-01/02, P9-RPC-01, P10-RPC-01/02/03/04/05, RPC-03/04/05/06,
D1-01, GX-07, etc.). There are:

- **0** uses of `unsafe`, `panic!`, `unimplemented!`, or `todo!`
- **0** `TODO` / `FIXME` comments
- **19 total `unwrap()`/`expect()` calls**, of which 10 are in `#[cfg(test)]`
  blocks, 4 are provably safe (constant or just-constructed values), and **5 are
  genuine production concerns** in `ws.rs`

The findings below are ordered by severity.

---

## Finding 1 — `Mutex::lock().unwrap()` in WebSocket handler (production panic risk)

| | |
|---|---|
| **Category** | Missing error handling |
| **Severity** | **Medium** |
| **Files** | `rpc/src/ws.rs` lines 456, 463, 480, 814 |

### Code

```rust
// ws.rs:456
let conns = IP_CONNECTIONS.lock().unwrap();

// ws.rs:463
return Response::builder()
    .status(429)
    .body(axum::body::Body::from("Too many connections from this IP"))
    .unwrap();

// ws.rs:480  (same pattern as 456)
// ws.rs:814
let mut conns = IP_CONNECTIONS.lock().unwrap();
```

### Risk

If a prior Mutex holder panics (poisoning the lock), every subsequent call to
`.lock().unwrap()` will panic too, cascading across all WebSocket handlers. Under
extreme load this can take down the entire WS subsystem.

### Recommendation

Replace `.lock().unwrap()` with `.lock().unwrap_or_else(|e| e.into_inner())`
(ignore poison) or switch to `parking_lot::Mutex` which does not poison. The
`Response::builder().unwrap()` calls are safe in practice (only fail with
conflicting status calls) but should use `expect("static builder")` for clarity.

---

## Finding 2 — `Response::builder().unwrap()` in WebSocket rejection path

| | |
|---|---|
| **Category** | Missing error handling |
| **Severity** | **Low** |
| **Files** | `rpc/src/ws.rs` lines 450, 463 |

### Code

```rust
// ws.rs:450
return Response::builder()
    .status(503)
    .body(axum::body::Body::from("Too many WebSocket connections"))
    .unwrap();
```

### Risk

`Response::builder()` only fails if the builder is in an invalid state (e.g.
setting status twice). Since the builder is constructed inline with a single
`.status()` call, this is **safe**. However, `.unwrap()` in production code is a
code-smell and could mask future refactoring errors.

### Recommendation

Replace with `.expect("static 503 response")` for documentation.

---

## Finding 3 — Prediction market `post_trade` returns preview-only status

| | |
|---|---|
| **Category** | Stub / placeholder behaviour |
| **Severity** | **Medium** |
| **File** | `rpc/src/prediction.rs` line 878, 958 |

### Code

```rust
// prediction.rs:878
/// In production this would create a transaction. For now returns the trade preview.
async fn post_trade(...) -> Response {
    // ...
    TradePreview {
        // ...
        status: "preview",
    }
}
```

### Risk

The comment says "for now" and the endpoint returns `status: "preview"`. A
client calling `POST /api/v1/prediction-market/trade` receives a 200 OK
response but **no trade is executed**. Front-ends that rely on this endpoint
will silently fail to submit trades.

### Recommendation

Either (a) return `405 Method Not Allowed` with a message directing users to
`sendTransaction` (matching the pattern used for `post_create` and DEX margin
POST endpoints), or (b) convert this into a transaction-template builder like
`post_create_template`.

---

## Finding 4 — Prediction market `post_create` intentionally disabled

| | |
|---|---|
| **Category** | Unwired endpoint (intentional) |
| **Severity** | **Low (Informational)** |
| **File** | `rpc/src/prediction.rs` lines 967–978 |

### Code

```rust
/// SECURITY: direct state writes are intentionally disabled.
async fn post_create(...) -> Response {
    let _ = state;
    let _ = req;
    api_err(
        "prediction-market/create is disabled for safety. Submit a signed transaction...",
    )
}
```

### Risk

None — this is a deliberate security decision documented inline. The route is
still registered (`POST /prediction-market/create`) so API discovery tools will
list it, but it correctly returns an error.

### Recommendation

No action required. Consider adding `#[deprecated]` or removing from the router
if it will never be enabled.

---

## Finding 5 — CORS defaults include `localhost` / `127.0.0.1`

| | |
|---|---|
| **Category** | Hardcoded data |
| **Severity** | **Low** |
| **File** | `rpc/src/lib.rs` lines 1740–1741 |

### Code

```rust
.unwrap_or_else(|| {
    vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "moltchain.io".to_string(),
        // ...
    ]
});
```

### Risk

The defaults include local origins. In production the `MOLTCHAIN_CORS_ORIGINS`
env var should be set. The code already has a hard abort if `*` is used on
mainnet (line 1752, `RPC-05`), which mitigates the worst case.

### Recommendation

No action strictly required. For defense in depth, consider removing
`localhost`/`127.0.0.1` from the default list and logging a warning when the env
var is not set.

---

## Finding 6 — EVM `eth_sendRawTransaction` uses `[0u8; 64]` placeholder signature

| | |
|---|---|
| **Category** | Placeholder / hardcoded data (intentional) |
| **Severity** | **Low (Informational)** |
| **File** | `rpc/src/lib.rs` line 9715 |

### Code

```rust
// AUDIT-FIX 2.15: Placeholder signature so downstream code doesn't reject
// as malformed. The actual ECDSA signature is inside the EVM transaction data.
signatures: vec![[0u8; 64]],
```

### Risk

This is **intentional and correct**. The EVM layer uses a sentinel blockhash
(`EVM_SENTINEL_BLOCKHASH`) so the processor routes to EVM execution which
has its own ECDSA verification. The placeholder exists only to satisfy the
`Transaction` struct invariant.

### Recommendation

No action required. The comment clearly documents the rationale.

---

## Finding 7 — Hardcoded prediction market program address

| | |
|---|---|
| **Category** | Hardcoded data |
| **Severity** | **Low** |
| **File** | `rpc/src/prediction.rs` line 30 |

### Code

```rust
const DEFAULT_PREDICT_PROGRAM_B58: &str = "J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD";
```

### Risk

If the prediction market contract is redeployed to a different address, the
hardcoded fallback will point to a stale program. However, the code first
resolves via symbol registry (`PREDICT`) and only falls back to this constant if
the registry lookup fails.

### Recommendation

Consider logging a warning when the fallback is used, or making it configurable
via env var.

---

## Finding 8 — Static `Mutex` caches in DEX REST handlers

| | |
|---|---|
| **Category** | Missing error handling / performance |
| **Severity** | **Low** |
| **File** | `rpc/src/dex.rs` lines 506, 511, 521 |

### Code

```rust
static SYMBOL_MAP_CACHE: Mutex<Option<(Instant, HashMap<String, String>)>> = Mutex::new(None);
static TICKERS_CACHE: Mutex<Option<(Instant, Vec<TickerJson>, u64)>> = Mutex::new(None);
static PAIR_ORDER_INDEX_CACHE: LazyLock<Mutex<HashMap<u64, PairOrderIndex>>> = ...;
```

### Risk

These are `std::sync::Mutex` caches accessed from an async context. Under high
request volume the lock will block the tokio thread while held, causing latency
spikes. If a holder panics, the cache poisons (same risk as Finding 1).

### Recommendation

Switch to `tokio::sync::Mutex` (async-aware) or `parking_lot::Mutex`
(non-poisoning, faster uncontended). For read-heavy caches, consider `RwLock`.

---

## Finding 9 — `requestAirdrop` performs direct state writes (bypassing consensus)

| | |
|---|---|
| **Category** | Security concern |
| **Severity** | **Medium** (mitigated) |
| **File** | `rpc/src/lib.rs` ~lines 11100–11200 |

### Code

```rust
require_single_validator(state, "requestAirdrop").await?;
// ... network_id check (testnet/devnet only) ...
// ... per-address rate limiting ...
// Direct state writes: debit treasury, credit recipient
```

### Risk

The airdrop handler directly mutates `StateStore` without going through
consensus. An attacker who gains RPC access to a single-validator testnet could
mint unlimited funds by bypassing the rate limiter (e.g., using multiple
addresses).

### Mitigations already in place

1. `require_single_validator` — disabled in multi-validator (production) mode
2. Network ID guard — only enabled on testnet/devnet
3. Per-address rate limiting with cooldown

### Recommendation

The guards are appropriate for a testnet faucet. Ensure the airdrop endpoint is
disabled or removed in mainnet builds as defense in depth.

---

## Finding 10 — `deployContract` / `upgradeContract` perform direct state writes

| | |
|---|---|
| **Category** | Security concern |
| **Severity** | **Medium** (mitigated) |
| **File** | `rpc/src/lib.rs` ~lines 6700–7400 |

### Code

```rust
require_single_validator(state, "deployContract").await?;
verify_admin_auth(state, &params)?;
// ... StateBatch direct writes ...
```

### Risk

Contract deployment and upgrade bypass consensus and write directly to state.
This is acceptable in single-validator mode but would break state consistency in
a multi-validator cluster.

### Mitigations already in place

1. `require_single_validator` — rejects if >1 validator is active
2. `verify_admin_auth` — constant-time admin token comparison

### Recommendation

Already well-guarded. Consider adding a log-level warning when these endpoints
are invoked.

---

## Finding 11 — `Box::leak(Box::new(dir))` in test helpers

| | |
|---|---|
| **Category** | Mock / test data in test paths |
| **Severity** | **Low (test-only)** |
| **Files** | `rpc/tests/rpc_handlers.rs` line 92, `rpc/tests/rpc_full_coverage.rs` line 74, `rpc/tests/shielded_handlers.rs` line 104 |

### Code

```rust
let dir = tempfile::tempdir().expect("tempdir");
let state = StateStore::open(dir.path()).expect("state");
let _ = Box::leak(Box::new(dir));
```

### Risk

`Box::leak` permanently leaks the `TempDir`, preventing cleanup. In tests this
is intentional (prevents early deletion while the router holds a reference) but
it means every test run leaks a temp directory.

### Recommendation

Acceptable for tests. If test parallelism grows, consider using
`Arc<TempDir>` + a custom `Drop` or a test harness that manages lifetimes.

---

## Finding 12 — `fake_sig` / `fake_hash` test data is test-only

| | |
|---|---|
| **Category** | Mock / test data |
| **Severity** | **None (test-only)** |
| **Files** | `rpc/tests/rpc_full_coverage.rs` lines 198, 1194, 1384, 1402, 1475, 1490, 1575 |

### Code

```rust
let fake_sig = "a".repeat(64);
let fake_hash = format!("0x{}", "a".repeat(64));
```

### Observation

All `fake_*` variables are confined to `#[tokio::test]` functions. None leak
into production code paths.

---

## Category Summary

| Category | Count | Severities |
|---|---|---|
| Stubs / placeholders | 1 | Medium (Finding 3) |
| Hardcoded data | 3 | Low (Findings 5, 6, 7) |
| TODO / FIXME comments | **0** | — |
| Unwired endpoints | 1 | Low / Informational (Finding 4) |
| Missing error handling | 3 | Medium (Finding 1), Low (Findings 2, 8) |
| Security concerns | 2 | Medium / mitigated (Findings 9, 10) |
| Mock / test data in production | **0** | — |
| Test-only observations | 2 | None (Findings 11, 12) |

---

## Files Audited

### Source files (`rpc/src/`)

| File | Lines | Issues |
|---|---|---|
| `lib.rs` | 12,718 | Findings 5, 6, 9, 10 |
| `ws.rs` | 1,734 | Findings 1, 2 |
| `dex.rs` | 2,836 | Finding 8 |
| `dex_ws.rs` | ~300 | None |
| `launchpad.rs` | 520 | None |
| `prediction.rs` | 1,438 | Findings 3, 4, 7 |
| `shielded.rs` | 1,405 | None |

### Test files (`rpc/tests/`)

| File | Lines | Issues |
|---|---|---|
| `compat_routes.rs` | 81 | None |
| `rpc_full_coverage.rs` | 2,893 | Finding 12 (test-only) |
| `rpc_handlers.rs` | 585 | Finding 11 (test-only) |
| `shielded_handlers.rs` | 925 | None |

---

## Overall Assessment

**Production readiness: HIGH**

The codebase demonstrates strong security practices:

- Tiered per-IP rate limiting (Cheap/Moderate/Expensive categories)
- Admin auth with constant-time comparison
- CORS origin validation with mainnet wildcard abort
- 2 MB body limit, 8,192 concurrency cap, 1 MB WebSocket message limit
- Bounded bincode deserialization preventing memory exhaustion
- EVM sentinel blockhash rejection preventing replay attacks
- Pre-mempool validation and preflight simulation
- Contract storage reads via `CF_CONTRACT_STORAGE` column family
- ZK proof self-verification with ark-bn254/Groth16
- Extensive `AUDIT-FIX` annotations from prior audit rounds
- Comprehensive test coverage (140+ integration tests across 4 test files)

The **5 actionable findings** (Findings 1, 3, 8, 9, 10) are all medium severity
or below, and Findings 9 and 10 are already well-mitigated by existing guards.
