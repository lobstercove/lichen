# Lichen — End-to-End Test Suite

> Complete reference for every E2E test file, what it covers, and how to run it.
> All integration, end-to-end, and audit tests live here. Rust unit tests remain
> co-located with their crates (`core/`, `rpc/`, `contracts/*/`, `sdk/rust/`).

---

## Prerequisites

### Running Cluster

All on-chain E2E tests require a running PQ-native validator cluster. The supported local smoke path is the canonical 3-validator launcher, which delegates to `run-validator.sh` and prepares the genesis DB via `lichen-genesis` automatically.

```bash
# 1. Build the required binaries
cargo build --release

# 2. Start a fresh 3-validator local cluster
export LICHEN_KEYPAIR_PASSWORD='local-e2e-secret'
bash scripts/start-local-3validators.sh start-reset

# 3. Verify RPC health
curl -s http://127.0.0.1:8899 \
	-H 'Content-Type: application/json' \
	-d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' | jq

# 4. Optional strict launch gate
STRICT_NO_SKIPS=1 bash scripts/launch-verification.sh
```

If you intentionally omit `LICHEN_KEYPAIR_PASSWORD`, the local launcher still works in explicit local-dev compatibility mode. That is useful for throwaway debugging, but it does not exercise the production encrypted-key path that the testnet and mainnet runbooks now require.

For broader local-stack scenarios that also need custody or faucet sidecars, start those services with your existing harness before running the suites that depend on them.

### Node.js Dependencies

```bash
npm install   # installs the shared JavaScript test dependencies
```

### Python Dependencies

```bash
pip install aiohttp requests nacl   # for Python tests
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RPC_URL` | `http://127.0.0.1:8899` | JSON-RPC endpoint |
| `REST_URL` | `http://127.0.0.1:8899` | REST API endpoint |
| `WS_URL` | `ws://127.0.0.1:8900` | WebSocket endpoint |
| `FAUCET_URL` | `http://127.0.0.1:9100` | Faucet endpoint |
| `FUND_AMOUNT` | `10` | LICN per funding request |
| `PREDICTION_MULTI_OUTCOME_ONLY` | `0` | Set to `1` to run only multi-outcome prediction tests |

---

## Quick Start

```bash
# Rust unit + contract tests
make test

# Full production gate (requires running 3-validator cluster)
STRICT_NO_SKIPS=1 bash scripts/launch-verification.sh

# Full matrix (dynamic total; adds custody withdrawal when the sidecar is healthy)
bash tests/run-full-matrix-feb24.sh

# Quick smoke check
python3 tests/quick-check.py
```

### Run ALL JavaScript E2E Tests

```bash
node tests/e2e-production.js
node tests/e2e-rpc-coverage.js
node tests/e2e-portal-interactions.js
node tests/e2e-wallet-flows.js
node tests/e2e-prediction.js
node tests/e2e-prediction-multi.js
node tests/e2e-dex.js
node tests/e2e-launchpad.js
node tests/e2e-transactions.js
node tests/e2e-volume.js
```

### Run ALL Python E2E Tests

```bash
python3 tests/e2e-dex-trading.py
python3 tests/e2e-user-services.py
python3 tests/e2e-custody-withdrawal.py
python3 tests/e2e-genesis-wiring.py
python3 tests/comprehensive-e2e.py
python3 tests/e2e-developer-lifecycle.py
python3 tests/contracts-write-e2e.py
```

### Run WebSocket Tests

```bash
node tests/test-ws-dex.js
node tests/test_ws_reconnect_stress.js
```

### Run Frontend Audit Tests (no running node needed)

```bash
node tests/test_frontend_asset_integrity.js
node tests/test_frontend_trust_boundaries.js
node tests/test_wallet_audit.js
node tests/test_wallet_extension_audit.js
node tests/test_wallet_modal_parity.js
node tests/test_marketplace_audit.js
node tests/test_website_audit.js
node tests/test_developers_audit.js
node tests/test_programs_override_wiring.js
node tests/test_coverage_audit.js
node tests/test_cross_cutting_audit.js
```

---

## Detailed Test File Reference

### On-Chain E2E Tests (require running cluster)

#### `e2e-production.js` — DEX Production E2E (220 tests)

The most comprehensive DEX test. Covers the full DEX lifecycle end-to-end.

```bash
node tests/e2e-production.js
```

| Section | Description |
|---------|-------------|
| P1 | Contract discovery via symbol registry |
| P2 | Multi-wallet funding (6 wallets via airdrop + faucet) |
| P3 | LichenID identity registration |
| P4 | DEX pair creation (on-chain TX) |
| P5 | Initial liquidity deposit |
| P6 | Multi-wallet trading (limit orders) |
| P7 | Price impact & order matching |
| P8 | REST API verification (markets, orderbook, trades) |
| P9 | WebSocket real-time subscriptions |
| P10 | Margin position creation |
| P11 | Margin collateral management |
| P12 | SporePump token launch lifecycle |
| P13 | Analytics & stats endpoints |
| P14 | Governance proposals |
| P15 | LP reward distribution |
| P16 | Advanced order types (stop-loss, take-profit) |
| P17 | Cross-pair trading |
| P18 | Final balance verification |
| P20 | Market orders (fill-or-kill) |
| P21 | Reduce-only orders |
| P22 | Close position via limit order |
| P23 | Partial close via limit order |
| P24 | Liquidation scenarios |
| P25 | AMM pool operations (add/remove liquidity, collect fees) |
| P26 | SporePump lifecycle (create → buy → sell → graduate) |
| P27 | Extended REST API endpoints |
| P28 | Extended RPC methods |
| P29 | Final balance verification |

---

#### `e2e-rpc-coverage.js` — Full RPC Method Coverage (117 tests)

Tests every single registered RPC method across all endpoints.

```bash
node tests/e2e-rpc-coverage.js
```

**Coverage:**
- Native RPC: 83 methods (getBalance, getTransaction, getBlock, getSlot, etc.)
- Solana-compatible: 9 methods (`/solana-compat` endpoint)
- EVM-compatible: 11 methods (`/evm` endpoint)
- REST endpoints: 24 routes (health, stats, markets, orderbook, etc.)

---

#### `e2e-wallet-flows.js` — Wallet User Flows (52 tests)

Simulates real wallet user journeys end-to-end.

```bash
node tests/e2e-wallet-flows.js
```

**Flows:** Keypair generation, airdrop funding, balance checking, LICN transfers, transaction history, LichenID registration, token creation & transfers, NFT minting, shielded pool (deposit/withdraw), EVM address registry, symbol registry queries, multi-wallet management.

---

#### `e2e-user-services.py` — Faucet, Explorer, Bridge, and Marketplace Service Flows (24 baseline checks + 4 custody checks when enabled)

Exercises real user-facing service interactions that sit outside the heavier protocol suites.

```bash
python3 tests/e2e-user-services.py
```

**Flows:** Faucet config/status/request/history/cooldown behavior, fresh transfer creation followed by explorer-style transaction/address/block browsing, validator/contracts/program listings, authenticated bridge deposit address creation and status lookup when custody is enabled, and marketplace browse RPC surfaces for listings, offers, auctions, and stats.

---

#### `e2e-custody-withdrawal.py` — Custody Withdrawal Burn Verification (14 checks)

Exercises the local custody withdrawal slice that the current dev stack can honestly prove end-to-end.

```bash
python3 tests/e2e-custody-withdrawal.py
```

**Flows:** Discovers the active genesis key directory, funds the wrapped-token admin if needed, mints wrapped tokens to a fresh user, creates a custody withdrawal job, burns the wrapped asset on-chain, submits the burn signature to custody, and polls audit events until `withdrawal.burn_confirmed` is observed.

---

#### `e2e-portal-interactions.js` — Explorer, Developers, and Programs Portal Runtime Flows

Verifies the runtime handoff between the public read-only portals and the Programs IDE without requiring a live browser harness.

```bash
node tests/e2e-portal-interactions.js
```

**Flows:** Explorer cross-app links are rewritten to the developer portal, the embedded Developers playground rewrites its fullscreen link and iframe to the Programs origin, and the Programs landing page copy/view-code controls drive clipboard writes and IDE navigation at runtime.

---

#### `e2e-prediction.js` — Prediction Market E2E (69 tests)

Full prediction market lifecycle: creation, trading, resolution, payouts.

```bash
node tests/e2e-prediction.js

# Multi-outcome markets only:
PREDICTION_MULTI_OUTCOME_ONLY=1 node tests/e2e-prediction.js
```

| Section | Description |
|---------|-------------|
| P1 | Contract discovery |
| P2 | Wallet funding (6 wallets) |
| P3 | LichenID identity registration |
| P4 | Market creation (binary 2-outcome + multi 3-outcome) |
| P5 | Initial liquidity provision with custom odds |
| P6 | Multi-wallet share purchases (YES/NO + multi-outcome) |
| P7 | Price impact verification (AMM invariant) |
| P8 | Share selling |
| P9 | Position verification |
| P10 | Analytics & stats |
| P11 | Edge cases & preflight gating |
| P12 | Price history & chart data |
| P13 | Complete-set mint/redeem operations |
| P14 | Liquidity management (add + withdraw) |
| P15 | Final state verification |
| P16 | Matrix expansion (binary + custom multi-outcome stress) |

---

#### `e2e-prediction-multi.js` — Multi-Outcome Prediction Deep Coverage (~70 tests)

Extended multi-outcome prediction market tests with full lifecycle coverage for 4, 5, 6, and 8-outcome markets.

```bash
node tests/e2e-prediction-multi.js
```

| Section | Description |
|---------|-------------|
| M1 | Contract & wallet setup |
| M2 | 4-outcome market creation & liquidity |
| M3 | 5-outcome market creation & liquidity |
| M4 | 6-outcome market creation & liquidity |
| M5 | 8-outcome market (max) creation & liquidity |
| M6 | Multi-outcome trading across all outcome indices |
| M7 | Multi-outcome sell shares |
| M8 | Multi-outcome complete-set operations |
| M9 | Multi-outcome price verification (sum ≈ 1.0) |
| M10 | Multi-outcome liquidity add/withdraw |
| M11 | Multi-outcome analytics & stats |
| M12 | Edge cases (invalid outcome index, zero amount, max outcomes) |
| M13 | REST API verification for multi-outcome markets |
| M14 | Final state & position verification |

---

#### `e2e-dex.js` — DEX Core E2E (82 tests)

Core DEX operations: pair creation, orders, matching, margin.

```bash
node tests/e2e-dex.js
```

---

#### `e2e-launchpad.js` — SporePump / Launchpad E2E (48 tests)

Token launch lifecycle via SporePump: create → bonding curve → buy → sell → graduation.

```bash
node tests/e2e-launchpad.js
```

---

#### `e2e-transactions.js` — Transaction Lifecycle (26 tests)

Core transaction operations: create, sign, send, confirm, history.

```bash
node tests/e2e-transactions.js
```

---

#### `e2e-volume.js` — Volume & Stress (116 tests)

High-volume trading scenarios, concurrent operations, throughput measurement.

```bash
node tests/e2e-volume.js
```

---

### WebSocket Tests

| File | Tests | Command |
|------|-------|---------|
| `test-ws-dex.js` | 1 | `node tests/test-ws-dex.js` |
| `test_ws_reconnect_stress.js` | 8 | `node tests/test_ws_reconnect_stress.js` |

---

### Python E2E Tests (require running cluster)

| File | Tests | Description |
|------|-------|-------------|
| `e2e-dex-trading.py` | 157 | DEX trading scenarios (pairs, swaps, liquidity) |
| `e2e-genesis-wiring.py` | 49 | Genesis block + initial state wiring for all 28 contracts |
| `comprehensive-e2e.py` | 2 | Sequential comprehensive E2E |
| `comprehensive-e2e-parallel.py` | — | Parallel version with asyncio |
| `e2e-developer-lifecycle.py` | 9 | Full developer workflow E2E |
| `contracts-write-e2e.py` | 176 | All 28 contracts — deploy, write, validate |
| `e2e-websocket-upgrade.py` | — | WebSocket upgrade and reconnection |
| `load-test-5k-traders.py` | — | Load test: 5000 concurrent traders |

> **Note:** contracts-write-e2e.py shows ~34 "no observable write delta" on pre-initialized genesis contracts — expected behavior, not bugs.

---

### Runners & Orchestrators

Top-level scripts that launch clusters and run multiple test suites:

| File | Description |
|------|-------------|
| `production-e2e-gate.sh` | **Production gate** — the canonical pass/fail gate |
| `run-full-matrix-feb24.sh` | **Full matrix** — runs the full automated suite set with reporting |
| `matrix-test-3val.sh` | 7-phase standalone matrix (RPC, CLI, WS, contracts, DEX, SDK, security) |
| `matrix-sdk-cluster.sh` | SDK cluster manager (start/stop/restart validators for matrix) |
| `multi-validator-e2e.sh` | Multi-validator consensus E2E |
| `live-e2e-test.sh` | Live 239-endpoint API sweep |
| `services-deep-e2e.sh` | Deep service integration (all frontends + backends) |
| `launch-3v.sh` | Helper: launch 3-validator cluster |
| `run-e2e.sh` | Helper: simple E2E launcher |

---

### Integration Tests (Shell)

Component-level tests against live services:

| File | Description |
|------|-------------|
| `test-cli-comprehensive.sh` | CLI command coverage |
| `test-contract-deployment.sh` | Contract deploy + verify |
| `test-rpc-comprehensive.sh` | RPC endpoint coverage |
| `test-websocket.sh` | WebSocket subscribe/stream |
| `test-dex-api-comprehensive.sh` | DEX REST API coverage |
| `test-critical-security.sh` | Security assertions |
| `test-mkt-featured-filter.sh` | Marketplace featured filter |

---

### Frontend Audit Tests (no running node needed)

These tests verify HTML/CSS/JS structure, wiring, and correctness of frontend files by parsing DOM structure.

| File | Tests | Description |
|------|-------|-------------|
| `test_wallet_audit.js` | 80 | Wallet web app structure & wiring |
| `test_wallet_extension_audit.js` | 80 | Wallet extension structure & wiring |
| `test_wallet_modal_parity.js` | 14 | Wallet modal parity between app & extension |
| `test_marketplace_audit.js` | 372 | Marketplace structure, 69k, 7 findings |
| `test_website_audit.js` | 101 | Website structure & content |
| `test_developers_audit.js` | 197 | Developer portal structure |
| `test_programs_override_wiring.js` | 11 | Programs IDE tab override wiring |
| `test_coverage_audit.js` | 33 | Overall coverage matrix audit |
| `test_cross_cutting_audit.js` | 44 | Cross-cutting concerns (auth, errors) |

---

### Utilities & Helpers

| File | Description |
|------|-------------|
| `helpers/funded-wallets.js` | Shared wallet generation & funding module |
| `_verify_fixes.js` | Quick verification of specific bug fixes |
| `debug-dex-order.js` | Debug DEX order building/signing |
| `debug-preflight.js` | Pre-flight connectivity check |
| `manual-margin-mode-e2e.js` | Manual margin mode testing |
| `resolve-funded-signers.py` | Resolve & fund genesis signers |
| `quick-check.py` | Quick smoke test |
| `update-expected-contracts.py` | Regenerate expected-contracts.json |
| `start-validator.sh` | Dev validator launcher (`--keep-state`, `--dev-mode`) |
| `expected-contracts.json` | Expected contract list for validation |

#### `tests/helpers/funded-wallets.js` — Shared Funding Helper

```javascript
const { loadFundedWallets, fundAccount, genKeypair, bs58encode, bs58decode, bytesToHex } = require('./helpers/funded-wallets');
```

| Function | Description |
|----------|-------------|
| `genKeypair()` | Returns a PQ-native wallet object `{ address, publicKey, seed }` |
| `fundAccount(address, amountLicn)` | Funds via airdrop (max 10 LICN chunks) with faucet fallback |
| `loadFundedWallets(count, amountEach)` | Returns array of funded wallets |
| `bs58encode(bytes)` / `bs58decode(str)` | Base58 encoding/decoding |
| `bytesToHex(bytes)` | Hex encoding |

---

## Funding Model

- **requestAirdrop**: Max 10 LICN per call, 60-second cooldown per address
- **Faucet HTTP**: POST to faucet endpoint, max 10 LICN, 60s cooldown, 150 LICN daily per IP
- **Genesis admin**: Loaded from `data/state-7001/genesis-keys/` or `keypairs/deployer.json`
- Tests that need >10 LICN chunk into multiple sequential airdrop calls with delays

## Contract Addresses

All 28 contracts are discovered at runtime via `getAllSymbolRegistry`. Key symbols:

| Symbol | Contract | Used By |
|--------|----------|---------|
| `PREDICT` | Prediction Market | e2e-prediction.js, e2e-prediction-multi.js |
| `DEX` | DEX Core | e2e-production.js, e2e-dex.js |
| `DEXAMM` | DEX AMM | e2e-production.js |
| `DEXROUTER` | DEX Router | e2e-production.js |
| `DEXMARGIN` | DEX Margin | e2e-production.js |
| `DEXREWARDS` | DEX Rewards | e2e-production.js |
| `DEXGOV` | DEX Governance | e2e-production.js |
| `SPOREPUMP` | SporePump Launchpad | e2e-launchpad.js, e2e-production.js |
| `ANALYTICS` | DEX Analytics | e2e-production.js |
| `YID` | LichenID | e2e-prediction.js, e2e-wallet-flows.js |
| `LUSD` | Lichen USD Stablecoin | e2e-prediction.js |
| `LICN` | LICN Token | e2e-wallet-flows.js |
| `ORACLE` | Oracle | e2e-production.js |

---

## Test Results Reference (v0.4.6)

| Test Suite | Pass | Fail | Skip |
|---|---|---|---|
| e2e-production.js | 220 | 0 | 3 |
| e2e-rpc-coverage.js | 117 | 0 | 0 |
| e2e-wallet-flows.js | 52 | 0 | 0 |
| e2e-prediction.js | 69 | 0 | 3 |
| e2e-prediction-multi.js | ~70 | 0 | ~3 |
| e2e-launchpad.js | 48 | 0 | 1 |
| e2e-dex.js | 82 | 0 | 3 |
| e2e-transactions.js | 26 | 0 | 0 |
| e2e-volume.js | 116 | 0 | 0 |
| test-ws-dex.js | 1 | 0 | 0 |
| test_ws_reconnect_stress.js | 8 | 0 | 0 |
| test_wallet_audit.js | 80 | 0 | 0 |
| test_wallet_extension_audit.js | 80 | 0 | 0 |
| test_wallet_modal_parity.js | 14 | 0 | 0 |
| test_marketplace_audit.js | 372 | 0 | 0 |
| test_website_audit.js | 101 | 0 | 0 |
| test_developers_audit.js | 197 | 0 | 0 |
| test_programs_override_wiring.js | 11 | 0 | 0 |
| test_coverage_audit.js | 33 | 0 | 0 |
| test_cross_cutting_audit.js | 44 | 0 | 0 |
| programs.test.js | 69 | 0 | 0 |
| e2e-dex-trading.py | 157 | 0 | 9 |
| e2e-genesis-wiring.py | 49 | 0 | 0 |
| comprehensive-e2e.py | 2 | 0 | 1 |
| e2e-developer-lifecycle.py | 9 | 0 | 2 |
| contracts-write-e2e.py | 176 | 34* | 0 |

\* 34 failures in contracts-write-e2e.py are "no observable write delta" on pre-initialized genesis contracts — expected behavior, not bugs.

**Total: ~2,000+ tests passing, 0 real failures.**

---

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Connection refused" | Cluster not running. Start validators and faucet first. |
| "Airdrop rate limited" | Wait 60s between requests. Tests handle this with faucet fallback. |
| "Contract not found in symbol registry" | Genesis wasn't run. Re-run genesis. |
| "no observable write delta" | Expected for pre-initialized genesis contracts. Not a real failure. |

---

## Artifacts

Test reports and run logs are written to `tests/artifacts/` (gitignored).

---

## Co-located Tests (not in this directory)

| Location | What |
|----------|------|
| `core/tests/` | Core blockchain Rust tests |
| `rpc/tests/` | RPC handler Rust tests |
| `contracts/*/tests/` | Per-contract adversarial Rust tests |
| `sdk/js/test*.js` | JavaScript SDK tests |
| `sdk/python/*test*.py` | Python SDK tests |
| `sdk/rust/examples/` | Rust SDK integration examples |
| `dex/dex.test.js` | DEX frontend test |
| `explorer/explorer.test.js` | Explorer frontend test |
| `faucet/faucet.test.js` | Faucet frontend test |
| `deploy/deploy.test.js` | Deployment integration test |
| `scripts/test-all-sdks.sh` | SDK cross-language test runner |
| `scripts/coverage_self_test.py` | Contract coverage self-test |
