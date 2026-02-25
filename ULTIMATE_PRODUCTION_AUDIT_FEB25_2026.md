# MoltChain Ultimate Production Audit — Feb 25, 2026

## Executive Summary

This audit is a deep production-readiness sweep across chain core, contracts, RPC/WS, SDKs, and frontend surfaces.

**Current verdict:** **NOT production-ready yet** for a full mainnet-style launch.

### Topline Health

- Full matrix: **26 pass / 11 fail** (`tests/run-full-matrix-feb24.sh`)
- DEX frontend assertions: **1879 pass / 0 fail** (`node dex/dex.test.js`)
- Wallet audits: **60/60 pass** (`node tests/test_wallet_audit.js`)
- Wallet extension audits: **70/70 pass** (`node tests/test_wallet_extension_audit.js`)
- RPC crate compile: **PASS** (`cargo check -p moltchain-rpc`)

## Scope & Methodology

This sweep combined:

1. **Dynamic validation**
   - `bash tests/run-full-matrix-feb24.sh`
   - `node dex/dex.test.js`
   - `node tests/test_wallet_audit.js`
   - `node tests/test_wallet_extension_audit.js`
   - `cargo check -p moltchain-rpc`

2. **Static risk scans**
   - Rust crash-prone patterns (`.unwrap()`, `.expect()`, `panic!()`)
   - Frontend production hygiene (`alert()`, `console.log`, `debugger`)
   - Debt markers (`TODO|FIXME|HACK|XXX|TBD`)

3. **Evidence extraction**
   - Direct review of `tests/artifacts/full_matrix_feb24_2026/full-matrix.log`

## Critical Blockers (P0)

### 1) Matrix instability across integration and SDK surfaces

**Evidence:** 11 failing suites in matrix report.

Failing suites:
- `tests/contracts-write-e2e.py`
- `tests/e2e-dex.js`
- `tests/e2e-volume.js`
- `tests/comprehensive-e2e.py`
- `tests/e2e-websocket-upgrade.py`
- `sdk/python/test_sdk_live.py`
- `sdk/python/test_websocket_sdk.py`
- `sdk/python/test_websocket_simple.py`
- `sdk/js/test-all-features.ts`
- `sdk/js/test-subscriptions.js`
- `sdk/rust/examples/test_transactions`

**Production risk:** release confidence is materially reduced because integration stability is inconsistent under the canonical matrix.

---

### 2) Environment/endpoint drift breaks SDK live coverage

**Evidence from logs:**
- Python SDK tests: `httpx.ConnectError: All connection attempts failed`
- JS SDK tests: repeated `fetch failed`
- WS SDK tests: `ECONNREFUSED ... :8900`
- Rust SDK example: `Connection refused` to `localhost:8899`

**Likely root cause:** test orchestration and SDK defaults are not consistently aligned with active cluster lifecycle/ports after multi-validator phases.

**Production risk:** client SDKs can appear unreliable even when node components are healthy.

---

### 3) Contract write and comprehensive API sweeps show state progression failures

**Evidence from logs:**
- `missing expected deployed contracts: shielded_pool,wbnb_token`
- large batches of contract calls failing with `RPC Error: No blocks yet`

**Production risk:** inconsistent chain progression/indexing visibility for contract call paths causes false negatives and potentially real runtime blind spots.

---

### 4) DEX trade visibility mismatch in E2E scenarios

**Evidence from logs:**
- `Orderbook has asks after Alice's sell order (0 asks)`
- `Trade history has entries (0 trades)`
- volume suite: `Pair 1 has trade history (0 trades)`

**Production risk:** execution can succeed while read-model/orderbook/trade history remains stale or incomplete; this is unacceptable for trading UX trust.

## High Priority Risks (P1)

### 5) Rust reliability debt density is high

Static scan count (`core`, `rpc`, `contracts`, `validator`, `cli`, `p2p`):
- **1874** occurrences of `.unwrap()/.expect()/panic!()`

Top concentration:
- `core/src`: 644
- `core/tests`: 426
- `rpc/tests`: 351
- `p2p/src`: 71
- `validator/src`: 36
- `rpc/src`: 16

**Production risk:** hard panics and unchecked assumptions can convert recoverable runtime issues into crashes.

---

### 6) Frontend production hygiene debt remains

Raw scan (`dex`, `wallet`, `website`, `explorer`):
- **238** occurrences of `alert()/console.log/debugger`

Production-code examples include:
- `wallet/js/wallet.js` (multiple blocking `alert()` flows)
- `wallet/extension/src/popup/popup.js` (blocking alerts in transaction/import paths)
- `website/script.js` (debug console logs)
- `dex/dex.js` (runtime console logs)

**Production risk:** blocking alerts degrade UX and scripted automation; noisy logs and debug paths make operational monitoring harder.

## Medium Risks (P2)

### 7) Test harness lifecycle coupling

The matrix shows phases where validators are terminated then subsequent live SDK checks still assume reachable RPC/WS defaults.

**Risk:** false negatives mask real regressions and waste release cycles.

---

### 8) Single documented technical debt marker in critical scan

One marker found (`dex/DEX_PLAN.md`) in docs only.

**Risk:** low directly, but indicates debt tracking mostly exists outside enforceable code checks.

## Domain-by-Domain Production Status

### Core / Validator / P2P
- Build/test signals are mixed; matrix has non-trivial integration failures.
- Reliability debt density (panic/unwrap/expect) is high.
- **Status:** Yellow/Red

### Contracts
- Many read/write methods fail in comprehensive sweeps with `No blocks yet`.
- Contract deployment assumptions differ across test phases (`shielded_pool`, `wbnb_token`).
- **Status:** Red

### RPC / WebSocket
- `cargo check -p moltchain-rpc` passes.
- Runtime access in matrix is inconsistent for some phases and SDK clients.
- **Status:** Yellow

### SDKs (Python / JS / Rust)
- Cross-SDK compatibility vectors pass.
- Live endpoint tests fail due connection lifecycle/endpoint assumptions.
- **Status:** Red

### DEX Frontend
- Local assertion suite now fully green (1879/1879).
- E2E still shows orderbook/trade-history consistency gaps.
- **Status:** Yellow

### Wallet + Extension Frontends
- Audit suites are fully green after latest pass.
- Import UX parity improved with per-word mnemonic grid + auto-paste in wallet/extension.
- **Status:** Green/Yellow (feature parity good; operational UX debt remains from alert-based flows)

## Fixes Applied During This Audit Pass

1. Restored full DEX wallet multi-tab production behavior (Wallets/Import/Extension/Create).
2. DEX now resets wallet modal inputs/state after successful wallet connect.
3. Updated DEX assertions to reflect restored multi-tab behavior.
4. Migrated mnemonic import UX to per-input grid with auto-paste in:
   - web wallet import flow
   - extension full-page import flow
   - extension popup import flow

## Final Remediation Backlog (Ordered)

## P0 (must fix before prod)

1. Stabilize matrix from 26/37 to deterministic pass gate (target: 37/37 or documented intentional skips).
2. Fix endpoint/cluster lifecycle alignment for SDK live tests (RPC/WS availability contract).
3. Resolve `No blocks yet` failure class in comprehensive contract/API sweeps.
4. Fix DEX read-model consistency so orderbook/trades reflect executed activity in E2E.

## P1 (high-value hardening)

5. Start systematic reduction of panic/unwrap/expect in runtime paths (`core/src`, `p2p/src`, `validator/src`, `rpc/src`).
6. Replace blocking `alert()` with non-blocking unified notification components in wallet + extension.
7. Remove/reduce production `console.log` noise and gate debug logs behind environment-level debug flags.

## P2 (quality + maintainability)

8. Segment matrix into deterministic profile lanes (single-validator, multi-validator, SDK-live) with explicit startup/teardown contracts.
9. Add CI artifact summarization for failing suites with root-cause tags (connectivity, indexing, deployment drift, assertion mismatch).

## Production Gate Recommendation

Use this gate before launch:

- **Gate A:** full matrix green (or approved skip-list with owner/date)
- **Gate B:** SDK live tests green on canonical local cluster profile
- **Gate C:** DEX E2E orderbook/trade-history consistency green
- **Gate D:** no new panic/unwrap/expect in runtime modules (ratchet policy)
- **Gate E:** no blocking `alert()` in production wallet/extension critical flows

Until P0 items are closed, production launch risk remains high.
