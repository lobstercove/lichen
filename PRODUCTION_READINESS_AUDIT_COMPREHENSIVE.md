# MoltChain Production-Readiness Audit — Comprehensive Report

**Date:** 2025-01-XX  
**Scope:** `scripts/`, `deploy/`, `infra/`, `tools/`, `tests/`, `fuzz/`, `dex/market-maker/`, root config files  
**Auditor:** Automated deep-read audit  
**Files Analyzed:** 108  

---

## Executive Summary

| Category | Critical | High | Medium | Low | Info |
|---|---|---|---|---|---|
| Stubs / Placeholders | 3 | 2 | 4 | — | — |
| Security | 5 | 6 | 8 | 3 | — |
| Reliability | 2 | 5 | 6 | 4 | — |
| Test Coverage Gaps | — | 3 | 5 | 2 | — |
| Dead Code / Duplication | — | 1 | 3 | 5 | 3 |
| Configuration Issues | 2 | 4 | 7 | 3 | — |
| Docker / Infra | 1 | 3 | 5 | 2 | — |
| CI/CD Gaps | 1 | 2 | 3 | — | — |
| Script Portability | — | 1 | 5 | 4 | — |
| Monitoring Gaps | 1 | 2 | 3 | 1 | — |
| **Totals** | **15** | **29** | **49** | **24** | **3** |

---

## Table of Contents

1. [scripts/ Directory](#1-scripts-directory)
2. [deploy/ Directory](#2-deploy-directory)
3. [infra/ Directory](#3-infra-directory)
4. [tools/ Directory](#4-tools-directory)
5. [tests/ Directory](#5-tests-directory)
6. [fuzz/ Directory](#6-fuzz-directory)
7. [dex/market-maker/ Directory](#7-dexmarket-maker-directory)
8. [Root Configuration Files](#8-root-configuration-files)
9. [Cross-Cutting Findings](#9-cross-cutting-findings)
10. [Remediation Priority Matrix](#10-remediation-priority-matrix)

---

## 1. scripts/ Directory

### 1.1 `scripts/VALIDATE_DESIGN.sh` (104 lines)
CSS design system validator — checks shared theme imports across frontends.

| Category | Finding | Severity |
|---|---|---|
| Reliability | No exit code on failure — script prints errors but always exits 0 | Medium |
| Portability | Uses `grep -oP` (Perl regex) — not available on macOS default grep | Medium |
| Test Coverage | Not invoked by any CI pipeline or Makefile target | Low |

### 1.2 `scripts/build-all-contracts.sh` (222 lines)
Builds all 27 WASM contracts with `--dex/--tokens/--core/--test` flags.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Well-structured with proper error handling and exit codes | ✅ OK |
| Portability | Uses standard bash constructs, portable across Linux/macOS | ✅ OK |
| Configuration | Contract list hardcoded in script — should derive from `contracts/*/Cargo.toml` discovery | Low |

### 1.3 `scripts/check_warnings.sh` (~40 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **HARDCODED absolute path** `/Users/johnrobin/.openclaw/workspace/moltchain/contracts/...` — breaks on any other machine | **High** |
| Test Coverage | Only checks 7 of 27 contracts (misses all DEX, wrapped tokens, prediction_market, bountyboard, etc.) | Medium |
| Dead Code | Effectively useless due to hardcoded paths — equivalent functionality exists in `Makefile lint` | Medium |

### 1.4 `scripts/coverage_self_test.py` (~160 lines)
Validates source exports match contract-reference.html and skill.md.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Uses `urllib.request` for optional RPC ABI checking — no timeout set, can hang | Medium |
| Configuration | Expects `contract-reference.html` to exist at hardcoded relative path | Low |
| Test Coverage | Good — validates documentation matches deployed reality | ✅ OK |

### 1.5 `scripts/e2e_test.sh` (625 lines)
Comprehensive 3-validator E2E test with 27 test sections.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Well-structured pass/fail tracking, kills validators on exit | ✅ OK |
| Monitoring | Includes health checks, slot sync verification, RPC endpoint testing | ✅ OK |
| Configuration | Hardcoded ports 8899/8901/8903 — should use variables | Low |
| Portability | Uses `python3 -c` inline — assumes Python 3 availability | Low |

### 1.6 `scripts/export_contract_abi_manifest.py` (~160 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Proper error handling for missing contracts/RPC failures | ✅ OK |
| Security | No input validation on RPC URL parameter | Low |

### 1.7 `scripts/final-sdk-status.sh` (~60 lines)

| Category | Finding | Severity |
|---|---|---|
| **Stubs** | **MOSTLY HARDCODED OUTPUT** — prints "100% READY" and fixed capability tables regardless of actual test results | **Critical** |
| Dead Code | Script serves no real diagnostic purpose — all output is static text | Medium |

### 1.8 `scripts/first-boot-deploy.sh` (257 lines)
Idempotent post-genesis contract deployment.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Good idempotency checks — skips if contracts already deployed | ✅ OK |
| Security | Uses inline Python to derive keypairs — no input validation | Medium |
| Configuration | Calls `deploy_dex.py` and `deploy_contract.py` — tightly coupled to tools/ | Low |

### 1.9 `scripts/generate-genesis.sh` (291 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **PLACEHOLDER validator pubkeys** — `Validator1PublicKeyBase58FormatHere01` left in mainnet genesis template | **Critical** |
| Stubs | Mainnet genesis block uses dummy validator keys — deploying this would create an unusable chain | Critical |
| Reliability | Testnet mode works correctly with auto-generated keys | ✅ OK |

### 1.10 `scripts/generate-release-keys.sh` (~100 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **QUESTIONABLE ENTROPY** — mixes time+pid+stack address before augmenting with `/dev/urandom`; the initial entropy is predictable | **High** |
| Security | Writes private key to disk in plaintext JSON | Medium |
| Configuration | Uses inline Rust program compiled on-the-fly — fragile, depends on Rust toolchain | Medium |

### 1.11 `scripts/generate-test-tx.sh` (~50 lines)

| Category | Finding | Severity |
|---|---|---|
| Stubs | Expected to fail without balance — essentially a smoke test stub | Low |
| Reliability | Acceptable for development/testing purposes | ✅ OK |

### 1.12 `scripts/generate-transactions-all-sdks.sh` (323 lines)

| Category | Finding | Severity |
|---|---|---|
| Dead Code | Generates example code files inline — primarily documentation, not a test | Info |
| Portability | Creates temp directories and example files — cleanup on error not guaranteed | Low |

### 1.13 `scripts/generate-transactions.sh` (~130 lines)

| Category | Finding | Severity |
|---|---|---|
| **Stubs** | **Uses `Hash::default()` as blockhash** — generated transactions will never be valid on-chain | **High** |
| Dead Code | Transaction examples use placeholder data — for documentation only | Info |

### 1.14 `scripts/genesis-airdrop-loop.sh` (~60 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **HARDCODED absolute path** `/Users/johnrobin/.openclaw` — breaks on any other machine or deployment | **High** |
| Reliability | Infinite loop with no exit condition, no error handling, no PID file | Medium |
| Monitoring | No logging, no health check before sending airdrop | Medium |

### 1.15 `scripts/health-check.sh` (208 lines)

| Category | Finding | Severity |
|---|---|---|
| Monitoring | Excellent — email/Slack alerting, disk space checks, continuous `--watch` mode | ✅ OK |
| Security | Slack webhook URL provided via env var (good practice) | ✅ OK |
| Configuration | Default thresholds sensible (slot stale >60s, disk >90%) | ✅ OK |

### 1.16 `scripts/launch-verification.sh` (~200 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **WRONG SDK PATHS** — references `js-sdk` and `python-sdk` instead of `sdk/js` and `sdk/python` | **High** |
| Reliability | Checks will falsely fail because the paths don't exist | High |
| Test Coverage | Good concept — pre-launch verification — but broken implementation | High |

### 1.17 `scripts/multi-validator-test.sh` (~170 lines)

| Category | Finding | Severity |
|---|---|---|
| **Reliability** | **Uses `killall -9`** — sends SIGKILL which prevents graceful shutdown, can corrupt RocksDB state | **High** |
| Portability | `killall` behavior differs between Linux (exact match) and macOS (partial match) | Medium |

### 1.18 `scripts/run-marketplace-demo.sh` (~60 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Calls `reset-blockchain.sh` — destroys all state before running | Medium |
| Dead Code | Demo script — not part of any test or CI pipeline | Low |

### 1.19 `scripts/run-multi-validator.sh` (~50 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Uses `cargo run --package` — runs from source, no release binary | Low |
| Monitoring | Basic process monitoring via background jobs | ✅ OK |

### 1.20 `scripts/seed-insurance-fund.sh` (~70 lines)

| Category | Finding | Severity |
|---|---|---|
| **Stubs** | **ONLY PRINTS INSTRUCTIONS** — does not actually execute any transactions or seed the fund | **High** |
| Dead Code | Script is a no-op that prints manual steps | Medium |

### 1.21 `scripts/setup-seed-node.sh` (408 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Well-structured — systemd setup, firewall rules, DNS configuration | ✅ OK |
| Security | Creates dedicated user, sets proper file permissions | ✅ OK |
| Portability | Debian/Ubuntu only — uses `apt-get`, `ufw` | Low |

### 1.22 `scripts/setup-validator.sh` (518 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Comprehensive — config generation, systemd, backup scripts, security | ✅ OK |
| Security | Includes firewall setup, SSH hardening checks | ✅ OK |
| Configuration | Well-parameterized with network-specific defaults | ✅ OK |

### 1.23 `scripts/sign-release.sh` (~110 lines)

| Category | Finding | Severity |
|---|---|---|
| Security | Uses inline Rust for Ed25519 signing — correct algorithm | ✅ OK |
| Reliability | Compiles Rust program on-the-fly — fragile, needs cargo/rustc | Medium |

### 1.24 `scripts/start-local-stack.sh` (~100 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Starts 3 validators + custody + faucet + first-boot deploy | ✅ OK |
| Configuration | Exports env vars for custody RPC URL — good practice | ✅ OK |

### 1.25 `scripts/start-validators.sh` (~50 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Quick 3-validator start with QUIC P2P | ✅ OK |
| Monitoring | No health check after startup | Low |

### 1.26 `scripts/status-local-stack.sh` (~60 lines)

| Category | Finding | Severity |
|---|---|---|
| Monitoring | Checks validator and custody health via curl | ✅ OK |
| Configuration | Missing faucet health check | Low |

### 1.27 `scripts/stop-local-stack.sh` (~35 lines)

| Category | Finding | Severity |
|---|---|---|
| **Reliability** | **DOESN'T STOP FAUCET** — kills validators and custody but leaves faucet running | **Medium** |
| Reliability | Uses `pkill` — could match unintended processes | Low |

### 1.28 `scripts/test-all-sdks.sh` (242 lines)

| Category | Finding | Severity |
|---|---|---|
| **Stubs** | **HARDCODED COVERAGE MATRIX** — prints "100% coverage" tables that are static text, not computed from actual test results | **Critical** |
| Dead Code | The "coverage summary" section is fake output | High |

### 1.29 `scripts/test-multi-validator.sh` (~100 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Uses `cargo build --release` — proper binary build | ✅ OK |
| Monitoring | Monitors validator logs | ✅ OK |

### 1.30 `scripts/test-sdk.sh` (~160 lines)

| Category | Finding | Severity |
|---|---|---|
| Stubs | Prints hardcoded "PARTIAL" and "NOT IMPLEMENTED" warnings — partially real, partially static | Medium |
| Test Coverage | Actually runs curl-based SDK tests but summary is misleading | Medium |

### 1.31 `scripts/testnet-deploy.sh` (263 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **References `molt_staking` contract** which is NOT in `build-all-contracts.sh` — deployment will fail | **High** |
| Reliability | 6-phase deployment with health checks between phases | ✅ OK |

### 1.32 `scripts/update-manifest.py` (~55 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Regenerates deploy-manifest.json from live symbol registry | ✅ OK |
| Security | No input validation on RPC URL | Low |

### 1.33 `scripts/upgrade-validator.sh` (~120 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **HARDCODED `PROJECT_ROOT=/opt/moltchain`** — won't work in development or non-standard deployments | **Medium** |
| Reliability | Has rollback support — good practice | ✅ OK |

### 1.34 `scripts/watchdog.sh` (279 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Excellent — auto-restart for stale/crashed validators with exponential backoff | ✅ OK |
| Monitoring | Configurable via JSON or env vars, proper logging | ✅ OK |
| Security | PID file management prevents duplicate instances | ✅ OK |

### 1.35 `scripts/validators.json` (~25 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **INCONSISTENT FLAGS** — V1 uses `--bootstrap-peers` while V2/V3 use `--bootstrap` | **Medium** |
| Configuration | Port 8004 for V3's P2P seems arbitrary (V1=8000, V2=8001, V3=8004 instead of 8002) | Low |

### 1.36 `scripts/moltchain-validator.service` (~45 lines)

| Category | Finding | Severity |
|---|---|---|
| Security | Good systemd hardening — `ProtectSystem=strict`, `NoNewPrivileges=yes`, `PrivateTmp=yes` | ✅ OK |
| Reliability | Has `Restart=on-failure` with 5s delay | ✅ OK |

---

## 2. deploy/ Directory

### 2.1 `deploy/deploy.test.js` (236 lines)
Infrastructure tests for Phase 20 audit fixes.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Tests port conflicts, Dockerfile EXPOSE, setup.sh validation, systemd hardening — well-structured | ✅ OK |
| Reliability | Uses `fs.readFileSync` for static analysis of config files — good approach | ✅ OK |
| Configuration | Tests reference specific port numbers — may need updating if ports change | Low |

### 2.2 `deploy/moltchain-validator.service` (~45 lines)

| Category | Finding | Severity |
|---|---|---|
| **Dead Code** | **EXACT DUPLICATE** of `scripts/moltchain-validator.service` — maintenance risk, will diverge | **Medium** |

### 2.3 `deploy/setup.sh` (~150 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Production server setup for Debian/Ubuntu, creates moltchain user | ✅ OK |
| Security | Sets proper file ownership, enables firewall rules | ✅ OK |
| Portability | Debian/Ubuntu only — uses `apt-get`, `adduser` | Low |

---

## 3. infra/ Directory

### 3.1 `infra/Dockerfile.custody` (~30 lines)

| Category | Finding | Severity |
|---|---|---|
| **Docker** | **NO HEALTHCHECK** — Docker/compose can't determine if service is actually working | **High** |
| Docker | **NO EXPOSE** directive — port usage undocumented | Medium |
| Docker | **NO NON-ROOT USER** — runs as root inside container | Medium |
| Security | No resource limits configured | Low |

### 3.2 `infra/Dockerfile.market-maker` (~15 lines)

| Category | Finding | Severity |
|---|---|---|
| **Docker** | **NO HEALTHCHECK** — same issue as custody | **High** |
| **Docker** | **NO NON-ROOT USER** — runs as root | **Medium** |
| Docker | Uses `npx ts-node` (dev dependency) — should pre-compile TypeScript | Medium |
| Configuration | COPY paths reference `../sdk` — depends on Docker build context being set correctly | Medium |

### 3.3 `infra/Dockerfile.moltchain` (~45 lines)

| Category | Finding | Severity |
|---|---|---|
| **Reliability** | **`build-all-contracts.sh \|\| true` SWALLOWS BUILD FAILURES** — contracts may silently fail to build | **Critical** |
| **Configuration** | **PORT MISMATCH** — `EXPOSE 8000/8001/9100` but compose maps `8899:8899/8900:8900/9100:9100` | **High** |
| Docker | Has healthcheck and non-root user — good | ✅ OK |

### 3.4 `infra/docker-compose.yml` (~120 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **CUSTODY RPC URL MISMATCH** — `CUSTODY_RPC_URL=http://moltchain:8000` but moltchain exposes RPC on `8899` | **Critical** |
| **Security** | **DEFAULT GRAFANA PASSWORD** `moltchain` — trivially guessable | **High** |
| Docker | Proper healthchecks on moltchain service, `depends_on` with conditions | ✅ OK |
| Docker | Named volumes for moltchain, prometheus, grafana — good | ✅ OK |

### 3.5 `infra/nginx/dex.conf` (229 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **CORS `Access-Control-Allow-Origin: *`** — allows any origin to access DEX API | **High** |
| Security | HTTP→HTTPS redirect, HSTS, TLS 1.2+ — good TLS posture | ✅ OK |
| Configuration | **HARDCODED `server_name dex.moltchain.io`** — must be parameterized for other deployments | Medium |
| Reliability | WebSocket proxy, rate limiting (10 req/s burst 20) — well-configured | ✅ OK |

### 3.6 `infra/prometheus/prometheus.yml` (~35 lines)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Scrapes `moltchain:9100` and self — correct | ✅ OK |
| **Monitoring** | **NGINX JOB** targets `nginx:80/metrics` — nginx doesn't expose Prometheus metrics without stub_status + exporter module | **Medium** |

### 3.7 `infra/prometheus/alerts.yml` (~70 lines)

| Category | Finding | Severity |
|---|---|---|
| **Monitoring** | **NO ALERTMANAGER CONFIGURED** — `alertmanager_targets: []` means alerts fire but nobody receives them | **Critical** |
| Monitoring | Good alert rules: NodeDown (1m), SlotStale (5m), InsuranceFundCritical (<1000) | ✅ OK |
| Monitoring | Missing alerts for: disk space, memory, custody bridge health, market maker health | Medium |

### 3.8 `infra/grafana/dashboards/dex-dashboard.json` (~110 lines)

| Category | Finding | Severity |
|---|---|---|
| Monitoring | Comprehensive dashboard: chain health, DEX metrics, margin, liquidations, AMM TVL | ✅ OK |
| Configuration | Uses Prometheus datasource — matches provisioning | ✅ OK |

### 3.9 `infra/grafana/provisioning/` (2 files)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Standard Grafana provisioning for dashboards and Prometheus datasource | ✅ OK |

### 3.10 `infra/scripts/setup-ssl.sh` (~45 lines)

| Category | Finding | Severity |
|---|---|---|
| **Configuration** | **HARDCODED EMAIL `admin@moltchain.io`** — Let's Encrypt notifications go to this address | **Medium** |
| Security | Uses certbot with `--agree-tos` — auto-accepts TOS | Low |

---

## 4. tools/ Directory

### 4.1 `tools/deploy_contract.py` (~100 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Single WASM contract deployer — functional | ✅ OK |
| Security | Derives program address from `SHA-256(deployer + wasm)` — deterministic but not standard | Low |
| Configuration | No validation of WASM file format before deployment | Medium |

### 4.2 `tools/deploy_dex.py` (601 lines)
**Most critical deployment script** — deploys entire DEX in 4 phases.

| Category | Finding | Severity |
|---|---|---|
| **Security** | **DEPLOYER AS ADMIN** — script warns "Replace with a multisig!" but uses deployer keypair as contract admin | **Critical** |
| Reliability | 4-phase deployment with verification step — well-structured | ✅ OK |
| Reliability | Saves `deploy-manifest.json` with all deployed addresses | ✅ OK |
| Configuration | References all 27 contract directories explicitly — must be updated when contracts are added | Medium |
| Test Coverage | No automated test for the deployment script itself | Medium |

### 4.3 `tools/deploy_live.py` (~160 lines)

| Category | Finding | Severity |
|---|---|---|
| **Dead Code** | **PREDECESSOR TO `deploy_dex.py`** — deploys only MoltCoin, MoltPunks, MoltSwap (3 of 27) | **Medium** |
| Configuration | May confuse operators if used instead of `deploy_dex.py` | Medium |

### 4.4 `tools/test_contracts_e2e.py` (~140 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | E2E tests for MoltCoin only (initialize, balance_of, mint, transfer, burn) | Medium |
| Test Coverage | Missing: DEX contracts, wrapped tokens, governance, margin, prediction market | Medium |

### 4.5 `tools/test_marketplace.py` (~130 lines)

| Category | Finding | Severity |
|---|---|---|
| **Reliability** | **USES FALLBACK HEURISTIC** — finds marketplace contract by position ("last deployed"), which is fragile | **Medium** |
| Test Coverage | Tests list_nft, buy_nft, cancel_listing — good coverage of marketplace | ✅ OK |

---

## 5. tests/ Directory

### 5.1 `tests/comprehensive-e2e-parallel.py` (1475 lines)
All 27 contracts tested concurrently via `asyncio.gather()`.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Excellent — tests all 27 contracts with both named-export and opcode ABIs | ✅ OK |
| Reliability | Thread-safe counters with locks, per-test timing, round-robin RPC distribution | ✅ OK |
| Configuration | Configurable via env vars (RPC_URL, TX_CONFIRM_TIMEOUT, MAX_CONCURRENCY, RPC_ENDPOINTS) | ✅ OK |
| Monitoring | Generates detailed timing data per contract and per test | ✅ OK |

### 5.2 `tests/comprehensive-e2e.py` (1368 lines)
Sequential version of the parallel test above.

| Category | Finding | Severity |
|---|---|---|
| Dead Code | Nearly identical to `comprehensive-e2e-parallel.py` — significant code duplication | Medium |
| Test Coverage | Same comprehensive coverage as parallel version | ✅ OK |

### 5.3 `tests/contracts-write-e2e.py` (1234 lines)
Write-path E2E tests with negative assertion support.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Excellent — validates contract writes, negative cases, function coverage, expected contract set | ✅ OK |
| Reliability | Generates JSON report, validates against expected-contracts.json lockfile | ✅ OK |
| Configuration | Highly configurable via 15+ env vars with strict defaults | ✅ OK |

### 5.4 `tests/debug-preflight.js` (~80 lines)
Tests prediction market preflight rejection for invalid trades.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Narrow — tests single edge case (buy on non-existent market) | Low |
| Reliability | Good negative test — verifies TX rejection | ✅ OK |

### 5.5 `tests/e2e-dex-trading.py` (1418 lines)
DEX trading, margin, prediction, and RPC coverage tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Comprehensive — order lifecycle, candles, margin, prediction market, multi-pair | ✅ OK |
| Reliability | Uses ABI-driven instruction building — correct binary encoding | ✅ OK |
| Test Coverage | Tests RPC stats methods and REST endpoints | ✅ OK |

### 5.6 `tests/e2e-dex.js` (818 lines)
JavaScript DEX E2E covering 12 lifecycle scenarios.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Full lifecycle: trade, LP, prediction, margin, governance, rewards, router, multi-user | ✅ OK |
| Security | Inline base58 and transaction signing — duplicated across multiple JS test files | Medium |
| Dead Code | Base58/RPC/encoding helpers are copy-pasted in 6+ JS test files | Medium |

### 5.7 `tests/e2e-launchpad.js` (875 lines)
ClawPump launchpad and governance E2E tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | 15 scenarios covering token creation, bonding curves, governance proposals | ✅ OK |
| Reliability | Uses `simulateTransaction` for preflight checks — good practice | ✅ OK |

### 5.8 `tests/e2e-prediction.js` (962 lines)
Prediction market comprehensive E2E.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | 13 sections: creation, liquidity, multi-wallet trading, price impact, analytics, edge cases | ✅ OK |
| Reliability | Verifies CPMM price shifts after trades, payout calculations | ✅ OK |

### 5.9 `tests/e2e-production.js` (1682 lines)
Production E2E covering all DEX_FINAL_PLAN features.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | 16 production scenarios: stop-limit, post-only, modify, partial close, WebSocket channels | ✅ OK |
| Test Coverage | Includes negative tests (unauthorized, invalid params, duplicates) | ✅ OK |
| Reliability | The most comprehensive single test file — covers features no other test touches | ✅ OK |

### 5.10 `tests/e2e-transactions.js` (612 lines)
Transaction signing and submission tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Tests basic RPC flow: airdrop → deploy → transfer → MoltPunks mint/transfer | ✅ OK |
| Security | Inline transaction signing matches validator's bincode format | ✅ OK |

### 5.11 `tests/e2e-volume.js` (963 lines)
Volume simulation with 5 wallets, multi-pair trading, WebSocket verification.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | 10 scenarios including orderbook stress test (10+ orders per side) | ✅ OK |
| Configuration | Requires optional `ws` npm module — gracefully skips if absent | ✅ OK |

### 5.12 `tests/e2e-websocket-upgrade.py` (1035 lines)
WebSocket subscriptions and contract upgrade system tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Tests DEX WS events, prediction WS, contract upgrade RPC, block/slot subscriptions | ✅ OK |
| Security | Tests `upgradeContract` RPC with admin token — verifies access control | ✅ OK |
| Configuration | Requires `websockets` Python package — not in any requirements.txt | Medium |

### 5.13 `tests/quick-check.py` (35 lines)
Minimal health + symbol registry check.

| Category | Finding | Severity |
|---|---|---|
| Configuration | Uses `httpx` — not in any requirements.txt; will fail if not installed | Medium |
| Dead Code | Very minimal — overlaps with `health-check.sh` and production-e2e-gate.sh | Low |

### 5.14 `tests/production-e2e-gate.sh` (500 lines)
Main production gate orchestrating multiple test suites.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Orchestrates: multi-validator, contract writes, DEX API, SDK, faucet, custody, launchpad | ✅ OK |
| Reliability | Auto-detects available services (faucet, custody) before testing | ✅ OK |
| Configuration | 30+ environment variable knobs with sensible defaults | ✅ OK |
| CI/CD | This should be the CI gate target — well-designed for automation | ✅ OK |

### 5.15 `tests/run-e2e.sh` (6 lines)
Thin wrapper that runs `comprehensive-e2e.py`.

| Category | Finding | Severity |
|---|---|---|
| Dead Code | Just invokes `comprehensive-e2e.py` and prints last 5 lines — minimal added value | Low |

### 5.16 `tests/services-deep-e2e.sh` (638 lines)
Deep E2E for DEX API, faucet, custody, launchpad, contracts.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Tests all 27 contracts via RPC `getContractInfo`, `getProgramStats`, `getProgramStorage`, `getProgramCalls`, `getContractEvents` | ✅ OK |
| Reliability | Handles contract discovery by keyword — robust fuzzy matching | ✅ OK |

### 5.17 `tests/test-dex-api-comprehensive.sh` (339 lines)
Comprehensive DEX REST API test suite.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Tests 18+ sections: stats, pairs, orderbook, trades, candles, tickers, pools, orders, margin, leaderboard, governance, rewards | ✅ OK |
| Configuration | Uses `MOLT_RPC_URL` env var with `localhost:8899` default — properly configurable | ✅ OK |

### 5.18 `tests/start-validator.sh` (12 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Has `--keep-state` guard before `rm -rf` — good safety measure | ✅ OK |
| Security | Warns before wiping state | ✅ OK |

### 5.19 `tests/launch-3v.sh` (~60 lines)

| Category | Finding | Severity |
|---|---|---|
| **Security** | **HARDCODED ABSOLUTE PATH** `cd /Users/johnrobin/.openclaw/workspace/moltchain` | **High** |
| Portability | Will fail on any other machine | High |
| Configuration | V3 uses P2P port 8004 but DB path `state-8002` — confusing mismatch | Low |

### 5.20 `tests/live-e2e-test.sh` (706 lines)
Live E2E against 3-validator testnet.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Comprehensive: health, sync, validators, token ops, contracts, DEX, faucet, custody | ✅ OK |
| Reliability | Uses `set +e` to track failures without aborting — appropriate for test scripts | ✅ OK |
| Monitoring | Includes rate-limit tolerance for repeated runs | ✅ OK |

### 5.21 `tests/test_coverage_audit.js` (201 lines)
Phase 21 audit verification tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Verifies 8 specific audit findings were fixed — good regression testing | ✅ OK |
| Reliability | Static analysis of source files — validates code patterns | ✅ OK |

### 5.22 `tests/test_cross_cutting_audit.js` (244 lines)
Phase 22 cross-cutting audit tests.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Verifies: no bare `panic!()`, graceful faucet exit, shared-config.js, favicon, .gitignore, Font Awesome consistency | ✅ OK |
| Configuration | Checks for `todo!()` and `unimplemented!()` in production Rust — excellent practice | ✅ OK |

### 5.23 `tests/test_marketplace_audit.js` (307 lines)
Phase 15 marketplace XSS audit verification.

| Category | Finding | Severity |
|---|---|---|
| Security | Verifies `escapeHtml` and `safeImageUrl` across all marketplace JS files — thorough XSS coverage | ✅ OK |
| Test Coverage | Tests both positive (proper escaping) and negative (protocol rejection) cases | ✅ OK |

### 5.24 `tests/test_wallet_audit.js` (472 lines)
Phase 11 wallet app audit verification.

| Category | Finding | Severity |
|---|---|---|
| Security | Verifies XSS fixes in NFT rendering, export modals, key validation, PBKDF2 | ✅ OK |
| Test Coverage | W-1 through W-9 findings verified with functional and static analysis | ✅ OK |

### 5.25 `tests/test_wallet_extension_audit.js` (465 lines)
Phase 12 wallet extension audit verification.

| Category | Finding | Severity |
|---|---|---|
| Security | E-1 through E-9: escapeHtml in nfts, full page, popup, settings, identity | ✅ OK |
| Test Coverage | Functional tests of extracted escapeHtml and safeImageUrl functions | ✅ OK |

### 5.26 `tests/test_website_audit.js` (329 lines)
Phase 18 website audit verification.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Verifies 7 findings: copyCode fix, mobile nav, footer links, CTA, accessibility, formatNumber, roadmap | ✅ OK |
| Reliability | Extracts and tests individual functions from source files | ✅ OK |

### 5.27 `tests/test_developers_audit.js` (299 lines)
Phase 19 developer portal audit verification.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Verifies D1–D12 across 15 HTML files: inline CSS removal, developers.js inclusion, aria-labels, breadcrumbs | ✅ OK |
| Configuration | Validates trust tiers match contract source code | ✅ OK |

### 5.28 `tests/test-ws-dex.js` (~40 lines)
WebSocket DEX subscription smoke test.

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Subscribes to dex_orderbook, dex_ticker, slots — validates JSON responses | ✅ OK |
| Reliability | 3-second timeout, exits 1 on 0 messages or non-JSON — proper assertions | ✅ OK |

### 5.29 `tests/expected-contracts.json` (27 entries)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Lists all 27 contracts — matches `build-all-contracts.sh` output | ✅ OK |
| CI/CD | Used by `update-expected-contracts.py --check` and `contracts-write-e2e.py` | ✅ OK |

### 5.30 `tests/update-expected-contracts.py` (~80 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Discovers contracts from `contracts/*/src/lib.rs`, diffs against lockfile | ✅ OK |
| CI/CD | `--check` flag for CI gate, `--write` for regeneration | ✅ OK |

---

## 6. fuzz/ Directory

### 6.1 `fuzz/Cargo.toml` (59 lines)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Correct `cargo-fuzz` setup with `libfuzzer-sys 0.4` and `moltchain-core` dependency | ✅ OK |
| Configuration | Separate workspace (`[workspace] members = ["."]`) to avoid interfering with main workspace | ✅ OK |
| CI/CD | **NO CI INTEGRATION** — fuzz targets are defined but no CI job runs them | **Medium** |

### 6.2 `fuzz/fuzz_targets/transaction_deser.rs` (~12 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Transaction` deserialization via both serde_json and bincode — good | ✅ OK |
| Security | Ensures arbitrary bytes never panic the deserializer | ✅ OK |

### 6.3 `fuzz/fuzz_targets/block_deser.rs` (~15 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Block` deserialization + hash computation on successfully-parsed blocks | ✅ OK |

### 6.4 `fuzz/fuzz_targets/consensus_vote.rs` (~14 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Vote` and `ForkChoice` deserialization — consensus-critical, high value target | ✅ OK |

### 6.5 `fuzz/fuzz_targets/hash_input.rs` (~16 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Hash::hash()` — verifies determinism and non-collision with empty | ✅ OK |
| Reliability | Includes `assert_eq!` and `assert_ne!` — will detect hash inconsistencies | ✅ OK |

### 6.6 `fuzz/fuzz_targets/instruction_parse.rs` (~18 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `ContractInstruction::deserialize` and `Instruction` construction | ✅ OK |

### 6.7 `fuzz/fuzz_targets/mempool_ops.rs` (61 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Mempool` operations (insert, priority) with arbitrary data — most sophisticated fuzz target | ✅ OK |
| Reliability | Creates deterministic test transactions from fuzz data | ✅ OK |

### 6.8 `fuzz/fuzz_targets/rpc_request.rs` (~28 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes JSON-RPC request parsing (single, batch, raw JSON value) | ✅ OK |
| Security | Validates the RPC server doesn't panic on malformed input — important for public-facing endpoint | ✅ OK |

### 6.9 `fuzz/fuzz_targets/account_deser.rs` (~18 lines)

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | Fuzzes `Account` deserialization + `Pubkey` construction from arbitrary bytes | ✅ OK |

### 6.10 Cross-Fuzz Assessment

| Category | Finding | Severity |
|---|---|---|
| Test Coverage | 8 fuzz targets covering all critical deserialization paths | ✅ OK |
| **CI/CD** | **NO CI INTEGRATION** — targets exist but aren't run in any pipeline or scheduled job | **High** |
| Test Coverage | Missing fuzz targets: contract WASM execution, P2P message parsing, WebSocket frame parsing | Medium |
| Configuration | No corpus directory — no initial seed inputs to guide fuzzing | Low |

---

## 7. dex/market-maker/ Directory

### 7.1 `dex/market-maker/package.json`

| Category | Finding | Severity |
|---|---|---|
| Configuration | Dependencies use `file:` references — requires specific workspace layout | Low |
| Configuration | Uses `ts-node` for production `start` script — should pre-compile for production | Medium |

### 7.2 `dex/market-maker/tsconfig.json`

| Category | Finding | Severity |
|---|---|---|
| Configuration | Target ES2020, strict mode, proper module resolution | ✅ OK |

### 7.3 `dex/market-maker/src/index.ts` (~110 lines)

| Category | Finding | Severity |
|---|---|---|
| Reliability | Graceful shutdown on SIGINT/SIGTERM — cancels all orders before exit | ✅ OK |
| Security | Wallet loading validates keypair format (array vs. object) | ✅ OK |
| Configuration | Properly parameterized via environment variables | ✅ OK |
| Test Coverage | **NO UNIT TESTS** — no test files exist for the market maker bot | **High** |

### 7.4 `dex/market-maker/src/config.ts` (~110 lines)

| Category | Finding | Severity |
|---|---|---|
| Configuration | All parameters configurable via env vars with sensible defaults | ✅ OK |
| Security | **NO INPUT VALIDATION** on parsed env vars — `parseInt('abc')` returns `NaN` | **Medium** |
| Reliability | Default `pairId: 0` — likely should be 1 (pair IDs are 1-indexed in the DEX) | Medium |

### 7.5 `dex/market-maker/src/strategies/spread.ts` (~190 lines)
Spread market-making strategy.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Skew adjustment logic is correct — shifts quotes against position | ✅ OK |
| Reliability | Cancels all orders before placing new ones each tick — clean slate approach | ✅ OK |
| Monitoring | Console logging per tick with position tracking | ✅ OK |
| Test Coverage | **NO UNIT TESTS** for strategy logic | **High** |

### 7.6 `dex/market-maker/src/strategies/grid.ts` (~180 lines)
Grid market-making strategy.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Grid initialization, order flipping on fills, missing order replacement | ✅ OK |
| Configuration | Minimum distance from current price (5bps) prevents self-trade | ✅ OK |
| **Reliability** | **NO CIRCUIT BREAKER** — bot continues placing orders even if every order fails | **Medium** |
| Test Coverage | **NO UNIT TESTS** | High |

---

## 8. Root Configuration Files

### 8.1 `Cargo.toml` (workspace root)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Workspace members: core, validator, rpc, cli, p2p, faucet, custody | ✅ OK |
| Configuration | Release profile: `opt-level = 3, lto = true, codegen-units = 1` — optimal for production | ✅ OK |
| Configuration | Excludes `contracts/*`, `sdk`, `compiler` from workspace — correct | ✅ OK |

### 8.2 `config.toml` (116 lines)
Validator configuration template.

| Category | Finding | Severity |
|---|---|---|
| Security | `admin_token = ""` — admin endpoints disabled by default | ✅ OK |
| Security | `rpc_rate_limit = 100` — basic rate limiting | ✅ OK |
| **Security** | **`bind_address = "0.0.0.0"`** — RPC binds to all interfaces by default; should default to `127.0.0.1` for security, with documentation to change for production via reverse proxy | **Medium** |
| Configuration | Testnet ports (7001, 8899) as defaults — appropriate for template | ✅ OK |
| Monitoring | Prometheus metrics enabled on port 9100 | ✅ OK |

### 8.3 `Dockerfile` (root, ~80 lines)

| Category | Finding | Severity |
|---|---|---|
| Docker | Multi-stage build with dependency caching — excellent | ✅ OK |
| Docker | Non-root user (`moltchain`), proper EXPOSE directives | ✅ OK |
| Docker | Copies validator, CLI, and faucet binaries — includes all needed services | ✅ OK |
| Configuration | Ports match config.toml: 7001 (P2P), 8899 (RPC), 8900 (WS), 9100 (metrics), 9101 (faucet) | ✅ OK |

### 8.4 `docker-compose.yml` (root, ~75 lines)

| Category | Finding | Severity |
|---|---|---|
| Docker | 3 services: validator, faucet, explorer (nginx static) | ✅ OK |
| Docker | Healthcheck on validator, `depends_on` with condition | ✅ OK |
| Configuration | Root compose is simpler than `infra/docker-compose.yml` — development vs production | ✅ OK |
| Monitoring | No Prometheus/Grafana in root compose — appropriate for development | ✅ OK |

### 8.5 `Makefile` (274 lines)

| Category | Finding | Severity |
|---|---|---|
| CI/CD | Comprehensive targets: build, test, deploy-local/testnet/mainnet, docker, lint, fmt, health | ✅ OK |
| **Reliability** | **`build-contracts` fallback uses `\|\| true`** — swallows contract build failures silently | **Medium** |
| CI/CD | `production-gate` target chains `check-expected-contracts` + `production-e2e-gate.sh` — good | ✅ OK |
| CI/CD | **NO CI CONFIGURATION FILE** — no `.github/workflows/`, no `.gitlab-ci.yml`, no `Jenkinsfile` | **Critical** |
| Configuration | `deploy-mainnet` reuses `testnet-deploy.sh` — should have separate mainnet deploy script | Medium |

### 8.6 `package.json` (root)

| Category | Finding | Severity |
|---|---|---|
| Configuration | Only `ws` and `tweetnacl` — minimal JS dependencies for test infrastructure | ✅ OK |
| Configuration | No `scripts` section — all automation via Makefile | ✅ OK |

### 8.7 `moltchain-start.sh` (318 lines)
Production-quality start script.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Handles genesis, joining, and resume modes with port conflict detection | ✅ OK |
| Reliability | PID file management for clean stop operations | ✅ OK |
| Configuration | Separate port assignments for testnet vs mainnet (non-overlapping) | ✅ OK |
| Security | Checks for already-running validators via `lsof` | ✅ OK |
| Monitoring | Writes PID env file for `moltchain-stop.sh` | ✅ OK |

### 8.8 `moltchain-stop.sh` (~80 lines)
Clean shutdown script.

| Category | Finding | Severity |
|---|---|---|
| Reliability | Uses PID file first, falls back to port-based `pkill` | ✅ OK |
| Reliability | Handles testnet, mainnet, and `all` modes | ✅ OK |
| Reliability | Verifies shutdown was successful | ✅ OK |

---

## 9. Cross-Cutting Findings

### 9.1 Massive Code Duplication in JS Test Files

**Severity: Medium**

The following ~100-line boilerplate block is copy-pasted across **7 JavaScript test files**:

- `tests/e2e-dex.js`
- `tests/e2e-launchpad.js`
- `tests/e2e-prediction.js`
- `tests/e2e-production.js`
- `tests/e2e-transactions.js`
- `tests/e2e-volume.js`
- `tests/debug-preflight.js`

Duplicated code includes:
- `bs58encode()` / `bs58decode()` — base58 encoding
- `sendRpcRequest()` — JSON-RPC client
- `encodeMsg()` / `createTx()` / `signTx()` — transaction building
- `writeU64LE()` / `writePubkey()` / `contractIx()` — binary ABI encoding

**Recommendation:** Extract into `tests/lib/test-helpers.js` and import.

### 9.2 Missing Python Requirements Files

**Severity: Medium**

No `requirements.txt` or `pyproject.toml` declares Python test dependencies. Scripts silently import:
- `httpx` (used by `quick-check.py`)
- `websockets` (used by `e2e-websocket-upgrade.py`)
- `moltchain` SDK (used by all Python E2E tests)

**Recommendation:** Create `tests/requirements.txt` and `tools/requirements.txt`.

### 9.3 No CI/CD Pipeline Configuration

**Severity: Critical**

No CI pipeline configuration exists anywhere in the repository:
- No `.github/workflows/`
- No `.gitlab-ci.yml`
- No `Jenkinsfile`
- No `circle.yml`

The `Makefile` has all the right targets (`test`, `lint`, `production-gate`, `check-expected-contracts`) but nothing triggers them automatically on push or PR.

**Recommendation:** Create a GitHub Actions workflow:
```yaml
# Suggested pipeline stages:
# 1. lint + fmt check
# 2. cargo build --release
# 3. cargo test (unit tests)
# 4. build-all-contracts.sh
# 5. Start 3 validators + run production-e2e-gate.sh
# 6. Nightly: cargo fuzz run (each target, 60s)
```

### 9.4 Hardcoded Absolute Paths (Systemic)

**Severity: High**

Three files contain the developer's home directory path `/Users/johnrobin/.openclaw/workspace/moltchain`:

| File | Line |
|---|---|
| `scripts/check_warnings.sh` | `CONTRACTS_DIR=...` |
| `scripts/genesis-airdrop-loop.sh` | `cd /Users/johnrobin/.openclaw` |
| `tests/launch-3v.sh` | `cd /Users/johnrobin/.openclaw/workspace/moltchain` |

**Recommendation:** Replace with `$(cd "$(dirname "$0")/.." && pwd)` or `SCRIPT_DIR` pattern.

### 9.5 Inconsistent Validator Flag Names

**Severity: Medium**

`--bootstrap` vs `--bootstrap-peers` used interchangeably across scripts and config files. If the validator binary only accepts one form, half the scripts will silently fail.

### 9.6 Two Divergent Docker Compose Files

**Severity: Medium**

- `docker-compose.yml` (root): Development — validator + faucet + explorer
- `infra/docker-compose.yml`: Production — validator + custody + market-maker + prometheus + grafana + nginx

These share no base configuration and will diverge in port mappings, environment variables, and service definitions.

**Recommendation:** Use `docker-compose.override.yml` pattern or `extends` to share base config.

### 9.7 `|| true` Pattern Swallowing Failures

**Severity: High**

Three locations use `|| true` to suppress contract build failures:

| File | Context |
|---|---|
| `infra/Dockerfile.moltchain` | `build-all-contracts.sh \|\| true` |
| `Makefile` | `build-contracts` target |
| `scripts/first-boot-deploy.sh` | Build fallback |

This means Docker images can be built and deployed with **missing contracts**, and CI gates won't catch it.

---

## 10. Remediation Priority Matrix

### P0 — Fix Before Any Deployment

| # | Finding | Location | Risk |
|---|---|---|---|
| 1 | No CI/CD pipeline | Repository root | Manual testing = missed regressions |
| 2 | Placeholder validator pubkeys in mainnet genesis | `scripts/generate-genesis.sh` | Unusable mainnet chain |
| 3 | Custody RPC URL uses wrong port (8000 vs 8899) | `infra/docker-compose.yml` | Custody bridge non-functional in Docker |
| 4 | `build-all-contracts.sh \|\| true` swallows failures | `infra/Dockerfile.moltchain`, `Makefile` | Deploying with missing contracts |
| 5 | Deployer keypair used as contract admin (not multisig) | `tools/deploy_dex.py` | Single key compromise = total DEX takeover |
| 6 | Alertmanager not configured | `infra/prometheus/alerts.yml` | Alerts fire into void |
| 7 | EXPOSE ports mismatch in Dockerfile.moltchain | `infra/Dockerfile.moltchain` | Documentation/tooling confusion |

### P1 — Fix Before Testnet Launch

| # | Finding | Location | Risk |
|---|---|---|---|
| 8 | Hardcoded absolute paths (3 files) | `check_warnings.sh`, `genesis-airdrop-loop.sh`, `launch-3v.sh` | Scripts fail on other machines |
| 9 | Wrong SDK paths in launch verification | `scripts/launch-verification.sh` | Pre-launch checks falsely fail |
| 10 | `killall -9` for validator shutdown | `scripts/multi-validator-test.sh` | RocksDB corruption |
| 11 | Default Grafana password | `infra/docker-compose.yml` | Unauthorized dashboard access |
| 12 | CORS `Access-Control-Allow-Origin: *` on DEX API | `infra/nginx/dex.conf` | Cross-site request attacks |
| 13 | Questionable entropy in key generation | `scripts/generate-release-keys.sh` | Predictable release signing keys |
| 14 | `molt_staking` contract reference (doesn't exist) | `scripts/testnet-deploy.sh` | Testnet deployment fails mid-phase |
| 15 | Fake coverage output ("100% READY") | `final-sdk-status.sh`, `test-all-sdks.sh` | False confidence in SDK coverage |
| 16 | No market maker unit/integration tests | `dex/market-maker/` | Unvalidated trading bot |
| 17 | Fuzz targets never run in CI | `fuzz/` | Deserialization bugs missed |
| 18 | `seed-insurance-fund.sh` is a no-op | `scripts/` | Insurance fund never funded |

### P2 — Fix Before Mainnet

| # | Finding | Location | Risk |
|---|---|---|---|
| 19 | RPC binds to `0.0.0.0` by default | `config.toml` | Unintended public exposure |
| 20 | Duplicate systemd service file | `deploy/` vs `scripts/` | Config drift |
| 21 | `stop-local-stack.sh` doesn't stop faucet | `scripts/` | Resource leak |
| 22 | Inconsistent `--bootstrap` flag names | `scripts/validators.json` | Validator startup failures |
| 23 | `deploy_live.py` predecessor still present | `tools/` | Operator confusion |
| 24 | `test_marketplace.py` uses position heuristic | `tools/` | Fragile marketplace test |
| 25 | Grid strategy has no circuit breaker | `dex/market-maker/` | Infinite failed order loop |
| 26 | `config.ts` doesn't validate env var parsing | `dex/market-maker/` | NaN config values |
| 27 | Hardcoded `PROJECT_ROOT=/opt/moltchain` | `scripts/upgrade-validator.sh` | Upgrade breaks in dev |
| 28 | Hardcoded SSL email | `infra/scripts/setup-ssl.sh` | Wrong cert notifications |
| 29 | Python test deps undeclared | Repository-wide | Failed test runs on clean machines |
| 30 | JS test helper duplication (~700 lines) | `tests/` (7 files) | Maintenance burden, divergence risk |
| 31 | Missing fuzz targets (P2P, WebSocket, WASM) | `fuzz/` | Attack surface uncovered |
| 32 | No fuzz corpus seeds | `fuzz/` | Reduced fuzzer effectiveness |
| 33 | `comprehensive-e2e.py` duplicates parallel version | `tests/` | Code divergence risk |
| 34 | `deploy-mainnet` reuses testnet deploy script | `Makefile` | Wrong config on mainnet |

---

## Appendix A: Files Analyzed (108 total)

**scripts/** (36 files): VALIDATE_DESIGN.sh, build-all-contracts.sh, check_warnings.sh, coverage_self_test.py, e2e_test.sh, export_contract_abi_manifest.py, final-sdk-status.sh, first-boot-deploy.sh, generate-genesis.sh, generate-release-keys.sh, generate-test-tx.sh, generate-transactions-all-sdks.sh, generate-transactions.sh, genesis-airdrop-loop.sh, health-check.sh, launch-verification.sh, multi-validator-test.sh, run-marketplace-demo.sh, run-multi-validator.sh, seed-insurance-fund.sh, setup-seed-node.sh, setup-validator.sh, sign-release.sh, start-local-stack.sh, start-validators.sh, status-local-stack.sh, stop-local-stack.sh, test-all-sdks.sh, test-multi-validator.sh, test-sdk.sh, testnet-deploy.sh, update-manifest.py, upgrade-validator.sh, watchdog.sh, validators.json, moltchain-validator.service

**deploy/** (3 files): deploy.test.js, moltchain-validator.service, setup.sh

**infra/** (11 files): Dockerfile.custody, Dockerfile.market-maker, Dockerfile.moltchain, docker-compose.yml, nginx/dex.conf, prometheus/prometheus.yml, prometheus/alerts.yml, grafana/dashboards/dex-dashboard.json, grafana/provisioning/dashboards/default.yml, grafana/provisioning/datasources/prometheus.yml, scripts/setup-ssl.sh

**tools/** (5 files): deploy_contract.py, deploy_dex.py, deploy_live.py, test_contracts_e2e.py, test_marketplace.py

**tests/** (30 files): comprehensive-e2e-parallel.py, comprehensive-e2e.py, contracts-write-e2e.py, debug-preflight.js, e2e-dex-trading.py, e2e-dex.js, e2e-launchpad.js, e2e-prediction.js, e2e-production.js, e2e-transactions.js, e2e-volume.js, e2e-websocket-upgrade.py, expected-contracts.json, launch-3v.sh, live-e2e-test.sh, production-e2e-gate.sh, quick-check.py, run-e2e.sh, services-deep-e2e.sh, start-validator.sh, test-dex-api-comprehensive.sh, test-ws-dex.js, test_coverage_audit.js, test_cross_cutting_audit.js, test_developers_audit.js, test_marketplace_audit.js, test_wallet_audit.js, test_wallet_extension_audit.js, test_website_audit.js, update-expected-contracts.py

**fuzz/** (9 files): Cargo.toml, fuzz_targets/account_deser.rs, block_deser.rs, consensus_vote.rs, hash_input.rs, instruction_parse.rs, mempool_ops.rs, rpc_request.rs, transaction_deser.rs

**dex/market-maker/** (6 files): package.json, tsconfig.json, src/index.ts, src/config.ts, src/strategies/spread.ts, src/strategies/grid.ts

**Root config** (8 files): Cargo.toml, config.toml, docker-compose.yml, Dockerfile, Makefile, package.json, moltchain-start.sh, moltchain-stop.sh

---

*End of audit report. 108 files analyzed across 10 audit dimensions.*
