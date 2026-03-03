# MoltChain Test Coverage Audit Report

**Date:** March 2, 2026  
**Scope:** All test suites, benchmarks, fuzz targets, e2e tests, frontend tests, SDK tests

---

## Executive Summary

MoltChain has **~22,300 lines** of test code across Rust integration tests, **~7,800 lines** of frontend JS tests, **~900 lines** of SDK tests, and **~180 lines** of fuzz targets. Core transaction processing and consensus have strong coverage (94 + 81 `#[test]` functions respectively), but significant gaps exist in **22 of 28 smart contracts** (no dedicated test suites), **P2P gossip protocol** (0 tests), **NFT/marketplace core modules** (0 tests), **RPC DEX/Launchpad/Prediction** submodules (0 inline tests), **WebSocket** (1 test), and all **SDK test files require a live validator** rather than running offline.

---

## 1. TEST COVERAGE MATRIX

### 1.1 Core Library (`core/src/`)

| Module | File | Lines | `#[test]` Count | Verdict |
|--------|------|-------|-----------------|---------|
| Account | `account.rs` | — | 3 | ⚠️ Minimal |
| Block | `block.rs` | — | 15 | ✅ Good |
| Consensus | `consensus.rs` | 3217+ | **94** | ✅ Excellent |
| Contract Runtime | `contract.rs` | 2015+ | 16 | ✅ Good |
| Contract Instruction | `contract_instruction.rs` | — | 3 | ⚠️ Minimal |
| Event Stream | `event_stream.rs` | — | 3 | ⚠️ Minimal |
| EVM Compat | `evm.rs` | 883+ | 6 | ⚠️ Light |
| Genesis | `genesis.rs` | — | 4 | ⚠️ Light |
| Hash | `hash.rs` | — | 2 | ⚠️ Minimal |
| Mempool | `mempool.rs` | — | 6 | ✅ OK |
| Multisig | `multisig.rs` | — | 3 | ⚠️ Minimal |
| Network Types | `network.rs` | — | 4 | ⚠️ Light |
| **NFT** | `nft.rs` | 96 | **0** | ❌ **NONE** |
| **Marketplace** | `marketplace.rs` | 47 | **0** | ❌ **NONE** |
| **lib.rs** | `lib.rs` | — | **0** | N/A (re-exports) |
| Processor | `processor.rs` | 4334+ | **81** | ✅ Excellent |
| ReefStake | `reefstake.rs` | — | 3 | ⚠️ Minimal |
| State Store | `state.rs` | 6582+ | 22 | ✅ Good |
| Transaction | `transaction.rs` | — | 9 | ✅ Good |

### 1.2 ZK Privacy (`core/src/zk/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `pedersen.rs` | 6 | ✅ Good |
| `merkle.rs` | 7 | ✅ Good |
| `note.rs` | 11 | ✅ Good |
| `keys.rs` | 5 | ✅ Good |
| `e2e_tests.rs` | 7 | ✅ Good |
| `prover.rs` | 0 | ⚠️ Tested via e2e only |
| `verifier.rs` | 0 | ⚠️ Tested via e2e only |
| `setup.rs` | 0 | ⚠️ Tested via e2e only |
| `circuits/` | 0 | ⚠️ Tested via e2e only |

ZK lifecycle integration test in `core/tests/zk_lifecycle.rs` (657 lines) covers full shield→unshield pipeline with real Groth16 proofs. Unit-level coverage is decent but `prover.rs`, `verifier.rs`, `setup.rs`, and `circuits/` have no standalone unit tests.

### 1.3 Core Integration Tests (`core/tests/`)

| Test File | Lines | Tests | Coverage Area |
|-----------|-------|-------|---------------|
| `basic_test.rs` | 131 | 7 | Keypair, hash, mempool, state, block basics |
| `adversarial_test.rs` | 636 | 12 | Double-spend, replay, overflow, slashing |
| `atomic_state.rs` | 298 | ~8 | Atomic WriteBatch, burn tracking |
| `activity_indexing.rs` | 145 | 3 | NFT activity, program calls, market activity |
| `caller_verification.rs` | 659 | 7 | Contract caller regression checks (source scan) |
| `contract_coverage.rs` | 1441 | ~60+ | All 27 contracts WASM load + deep tests for 6 |
| `contract_lifecycle.rs` | 478 | ~8 | Deploy→init→call→query pipeline |
| `cross_contract_call.rs` | 423 | ~8 | CCC host import pipeline |
| `production_readiness.rs` | 1635 | **101** | Block storage, accounts, fees, slashing, fork choice, voting |
| `wire_format.rs` | 323 | 8 | Bincode/JSON cross-SDK compatibility |
| `zk_lifecycle.rs` | 658 | ~5 | Full ZK shield/unshield with Groth16 |

### 1.4 Benchmarks (`core/benches/`)

| File | Lines | Benchmarks |
|------|-------|-----------|
| `processor_bench.rs` | 190 | TX throughput (1/10/50), block creation (0/10/100/500 tx), state store ops, hash |

✅ Benchmarks exist but only cover basic throughput. **Missing:** contract execution benchmarks, ZK proof generation benchmarks, mempool contention benchmarks.

### 1.5 Fuzz Targets (`fuzz/fuzz_targets/`)

| Target | Lines | What It Fuzzes |
|--------|-------|---------------|
| `account_deser.rs` | 17 | Account JSON + bincode deser |
| `block_deser.rs` | 14 | Block JSON + bincode deser |
| `consensus_vote.rs` | 13 | Vote + ForkChoice deser |
| `hash_input.rs` | 18 | Hash determinism |
| `instruction_parse.rs` | 19 | ContractInstruction deser |
| `mempool_ops.rs` | 61 | Mempool add/drain ops |
| `rpc_request.rs` | 25 | JSON-RPC request parsing |
| `transaction_deser.rs` | 12 | Transaction deser |

✅ Good coverage of deserialization panic resistance. **Missing fuzz targets:** contract WASM execution, signature verification, ZK proof verification, P2P message parsing, EVM input parsing.

### 1.6 RPC Tests (`rpc/tests/` + `rpc/src/`)

| File | Tests | Coverage |
|------|-------|---------|
| `rpc/tests/rpc_full_coverage.rs` | **200** async tests | All 108 native + 13 Solana-compat + 20 EVM-compat methods + REST API |
| `rpc/tests/rpc_handlers.rs` | ~15 | Core JSON-RPC endpoints |
| `rpc/tests/compat_routes.rs` | 2 | Solana health + EVM chainId |
| `rpc/tests/shielded_handlers.rs` | **36** | Shielded pool RPC + REST |
| `rpc/src/lib.rs` (inline) | 16 | Router construction, method dispatch |
| `rpc/src/ws.rs` (inline) | **1** | ⚠️ Only 1 WebSocket test |
| `rpc/src/shielded.rs` (inline) | 8 | Shielded module |
| **`rpc/src/dex.rs`** | **0** | ❌ **NO TESTS** (2000+ line file) |
| **`rpc/src/dex_ws.rs`** | **0** | ❌ **NO TESTS** |
| **`rpc/src/launchpad.rs`** | **0** | ❌ **NO TESTS** |
| **`rpc/src/prediction.rs`** | **0** | ❌ **NO TESTS** |

### 1.7 P2P Networking (`p2p/src/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `peer.rs` | 22 | ✅ Good (TLS, connection, cert verify) |
| `network.rs` | 10 | ✅ Good (config, role defaults) |
| `peer_store.rs` | 5 | ✅ Good |
| `peer_ban.rs` | 7 | ✅ Good |
| `message.rs` | 3 | ⚠️ Light |
| **`gossip.rs`** | **0** | ❌ **NO TESTS** (critical protocol) |

### 1.8 Validator (`validator/src/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `main.rs` | 2 | ⚠️ Very light for 12k+ line file |
| `sync.rs` | 4 | ⚠️ Light |
| `keypair_loader.rs` | 1 | ⚠️ Minimal |
| `threshold_signer.rs` | 5 | ✅ OK |
| `updater.rs` | 11 | ✅ Good |

### 1.9 CLI (`cli/src/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `keygen.rs` | 9 | ✅ Good |
| `main.rs` | 5 | ⚠️ Light (1800+ line file) |

Shell-based CLI integration tests exist (`test-cli-comprehensive.sh`) but require a running validator.

### 1.10 Custody (`custody/src/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `main.rs` | ~20+ | ✅ Good |

### 1.11 Compiler (`compiler/src/`)

| Module | `#[test]` Count | Verdict |
|--------|-----------------|---------|
| `main.rs` | 18 | ✅ Good |

### 1.12 SDK Tests

| SDK | Test Files | Offline? | Verdict |
|-----|-----------|----------|---------|
| **JS** | `test.js` (18L), `test_cross_sdk_compat.js` (127L), `test_bincode_format.js` (93L), `test-subscriptions.js` (57L) | compat only | ⚠️ Cross-SDK compat is offline; live SDK tests need validator |
| **Python** | `test_bincode.py` (60L), `test_cross_sdk_compat.py` (80L), `test_sdk_live.py` (129L), `test_websocket_sdk.py` (68L), `test_websocket_simple.py` (54L) | compat only | ⚠️ Same pattern |
| **Rust** | `types.rs` (6 inline tests), `client.rs` (4 inline tests), `examples/comprehensive_test.rs` | inline only | ⚠️ No integration test suite; example file needs live validator |

### 1.13 Frontend JS Tests

| File | Lines | Coverage |
|------|-------|---------|
| `explorer/explorer.test.js` | 440 | XSS prevention, utility funcs, trust tiers |
| `dex/dex.test.js` | **6015** | escapeHtml, bs58, bincode, wallet-gates, oracle, trade bridge |
| `faucet/faucet.test.js` | 216 | XSS, address validation, docker config |
| `deploy/deploy.test.js` | 236 | Docker ports, Dockerfile, systemd service |
| `monitoring/monitoring.test.js` | 327 | Monitoring dashboard |
| `tests/test_wallet_audit.js` | 865 | Wallet app 9 findings |
| `tests/test_wallet_extension_audit.js` | 542 | Wallet extension 9 findings |
| `tests/test_marketplace_audit.js` | 847 | Marketplace 7 findings |
| `tests/test_website_audit.js` | 347 | Website 7 findings |
| `tests/test_developers_audit.js` | 325 | Developer portal |
| `tests/test_coverage_audit.js` | 201 | Meta-coverage |
| `tests/test_cross_cutting_audit.js` | 243 | Cross-cutting concerns |
| `tests/test_wallet_modal_parity.js` | 120 | Modal parity DEX/marketplace/programs |
| `tests/test_programs_override_wiring.js` | 43 | Programs override wiring |
| `tests/test_ws_reconnect_stress.js` | 397 | WebSocket reconnect stress (needs live server) |

### 1.14 E2E / Integration Tests (`tests/`)

| Type | Files | Notes |
|------|-------|-------|
| Rust e2e | `tests/e2e/tests/e2e.rs` (577L) | Full DEX+token pipeline via test_mock |
| Python e2e | `comprehensive-e2e.py`, `e2e-dex-trading.py`, `e2e-genesis-wiring.py`, etc. | All require running validator |
| JS e2e | `e2e-dex.js`, `e2e-transactions.js`, `e2e-production.js`, etc. | All require running validator |
| Shell e2e | `production-e2e-gate.sh`, `multi-validator-e2e.sh`, etc. | Infrastructure tests |

---

## 2. SMART CONTRACT COVERAGE MATRIX

| # | Contract | Dedicated Test Suite | contract_coverage.rs | caller_verification.rs | E2E Coverage | Verdict |
|---|----------|---------------------|---------------------|----------------------|-------------|---------|
| 1 | bountyboard | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 2 | clawpay | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 3 | clawpump | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 4 | clawvault | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 5 | compute_market | ❌ | ✅ **Deep** (33 funcs) | ❌ | ❌ | ✅ Good |
| 6 | dex_amm | ✅ `tests/adversarial.rs` (15k) | ✅ Load + init | ❌ | ✅ e2e.rs | ✅ Good |
| 7 | dex_analytics | ❌ | ✅ Load + init | ❌ | ✅ e2e.rs | ⚠️ Light |
| 8 | dex_core | ✅ `tests/adversarial.rs` (16k) | ✅ Load + init | ❌ | ✅ e2e.rs | ✅ Good |
| 9 | dex_governance | ❌ | ✅ **Deep** (17 funcs) | ❌ | ✅ e2e.rs | ✅ Good |
| 10 | dex_margin | ✅ `tests/adversarial.rs` (17k) | ✅ Load + init | ❌ | ✅ e2e.rs | ✅ Good |
| 11 | dex_rewards | ❌ | ✅ Load + init | ✅ initialize | ✅ e2e.rs | ⚠️ Medium |
| 12 | dex_router | ❌ | ✅ Load + init | ❌ | ✅ e2e.rs | ⚠️ Light |
| 13 | lobsterlend | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 14 | moltauction | ❌ | ✅ Load + init | ✅ create_auction | ❌ | ⚠️ Light |
| 15 | moltbridge | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 16 | moltcoin | ✅ `test_contract.py` (4k) | ✅ Load + init | ✅ approve, mint | ❌ | ✅ Good |
| 17 | moltdao | ❌ | ✅ **Deep** (25 funcs) | ✅ cancel_proposal | ❌ | ✅ Good |
| 18 | moltmarket | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 19 | moltoracle | ❌ | ✅ **Deep** (24 funcs) | ✅ submit_price | ❌ | ✅ Good |
| 20 | moltpunks | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 21 | moltswap | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 22 | moltyid | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 23 | musd_token | ✅ `tests/adversarial.rs` (18k) | ✅ Load + init | ❌ | ✅ e2e.rs | ✅ Good |
| 24 | prediction_market | ✅ `tests/` (3 files, 104k!) | ✅ Load + init | ❌ | ❌ | ✅ Excellent |
| 25 | reef_storage | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 26 | shielded_pool | ❌ | ✅ Load + init | ❌ | ✅ ZK lifecycle | ⚠️ Medium |
| 27 | wbnb_token | ❌ | ✅ Load + init | ❌ | ❌ | ⚠️ Light |
| 28 | weth_token | ❌ | ✅ **Deep** (21 funcs) | ❌ | ❌ | ✅ Good |
| 29 | wsol_token | ❌ | ✅ **Deep** (21 funcs) | ❌ | ❌ | ✅ Good |

**Summary:** 6/29 contracts have dedicated test suites. 7 contracts have deep function-level WASM coverage via `contract_coverage.rs`. 16 contracts only have load+initialize-level coverage.

---

## 3. CRITICAL FINDINGS

### 3.1 Tests That Are Skipped/Disabled/Commented Out

**No `#[ignore]` or `#[should_panic]` annotations found** across any Rust test files. No JS tests use `.skip()` or `xit()`. This is positive — no tests are silently disabled.

### 3.2 Tests Using Mock Data That May Not Match Production

| Test | Issue | Severity |
|------|-------|----------|
| `contract_coverage.rs` all WASM tests | Use synthetic storage (`HashMap`) with hardcoded admin keys `[1u8;32]` — never tests with real deployed contract storage layout | **Medium** |
| `caller_verification.rs` | Only does **source-code string matching** (checks if specific strings appear in `.rs` files) — not actual execution-based verification | **High** |
| All JS frontend tests | **Re-implement functions from source** rather than importing them — if implementation drifts from the re-implementation, tests pass falsely | **High** |
| `adversarial_test.rs` | Uses minimal WASM (`[0x00, 0x61, 0x73, 0x6d, ...]`) that doesn't represent real contracts | **Low** (testing processor behavior, not contracts) |
| SDK live tests | Hard-code expected addresses and validator pubkeys | **Low** |

### 3.3 Missing Edge Case Tests

| Area | Missing Edge Cases |
|------|-------------------|
| **Mempool** | Only 6 tests — missing: full mempool eviction under load, fee priority ordering with ties, transaction expiry, concurrent add/drain race conditions |
| **Account** | Only 3 tests — missing: data field size limits, executable account modification restrictions, rent-exempt minimum |
| **Hash** | Only 2 tests — missing: collision resistance validation, large input performance |
| **Consensus** | Strong (94 tests) but missing: network partition simulation, Byzantine fault scenarios beyond double-vote |
| **EVM** | Only 6 tests — missing: all ERC-20 methods, gas estimation edge cases, contract creation via EVM, address translation validation |
| **ReefStake** | Only 3 tests — missing: partial unstake, reward distribution, epoch boundary, slashing during stake |
| **Multisig** | Only 3 tests — missing: threshold changes, owner rotation, concurrent approval races |
| **Genesis** | Only 4 tests — missing: invalid genesis data, genesis with maximum validators, duplicate account handling |

### 3.4 Missing E2E/Integration Test Scenarios

1. **Multi-validator consensus** — `multi-validator-e2e.sh` exists but no Rust-level multi-validator test
2. **Chain reorg/rollback** — fork choice is tested in `production_readiness.rs` but no real chain reorg simulation
3. **P2P network partition** — no test for split-brain recovery
4. **Cross-contract call failure cascading** — `cross_contract_call.rs` tests happy path but limited failure cascading
5. **Validator join/leave during active consensus** — untested
6. **State snapshot/restore** — no test for validator state sync from snapshot
7. **Maximum block size** — no test for block with maximum allowed transactions
8. **Long-running stability** — no soak test or stress test for memory leaks

### 3.5 Test Files With No Actual Test Content

None found — all test files contain actual executable tests.

---

## 4. COMPONENT COVERAGE SUMMARY

| Component | Unit Tests | Integration Tests | E2E Tests | Fuzz Tests | Verdict |
|-----------|-----------|-------------------|-----------|-----------|---------|
| **Transaction Processing** | 81 | ✅ lifecycle, adversarial | ✅ e2e.rs | ✅ | ✅ Well-covered |
| **Consensus/Voting** | 94 | ✅ production_readiness | ⚠️ Shell only | ✅ vote deser | ✅ Well-covered |
| **State Store** | 22 | ✅ atomic_state | ✅ | ❌ | ✅ Good |
| **Block Production** | 15 | ✅ | ✅ | ✅ block deser | ✅ Good |
| **Smart Contracts** | 16 (runtime) | ✅ 60+ WASM tests | ✅ e2e.rs | ❌ | ⚠️ 16/29 contracts under-tested |
| **ZK Proof System** | 36 (across zk/) | ✅ zk_lifecycle.rs | ❌ | ❌ | ✅ Good (missing ZK fuzz) |
| **P2P Networking** | 47 | ❌ | ⚠️ Shell only | ❌ | ⚠️ No gossip tests, no network-level integration |
| **RPC Endpoints** | 25 | ✅ 200+ handler tests | ⚠️ Shell/Python | ✅ rpc_request | ✅ Well-covered |
| **WebSocket** | **1** | ⚠️ stress test (live only) | ⚠️ JS live only | ❌ | ❌ **Severely under-tested** |
| **CLI Commands** | 14 | ⚠️ Shell (live only) | ⚠️ Shell (live only) | ❌ | ⚠️ Light offline coverage |
| **Explorer Frontend** | N/A | 440L JS tests | ❌ | ❌ | ⚠️ Tests re-implement funcs |
| **DEX Frontend** | N/A | 6015L JS tests | ✅ JS e2e | ❌ | ✅ Good (same re-impl caveat) |
| **Wallet App** | N/A | 865L + 542L JS tests | ❌ | ❌ | ⚠️ Tests re-implement funcs |
| **Marketplace Frontend** | N/A | 847L JS tests | ❌ | ❌ | ⚠️ Tests re-implement funcs |
| **Website** | N/A | 347L JS tests | ❌ | ❌ | ⚠️ Tests re-implement funcs |
| **JS SDK** | N/A | 238L compat tests offline | ⚠️ Live only | ❌ | ⚠️ No offline integration |
| **Python SDK** | N/A | 140L compat tests offline | ⚠️ Live only | ❌ | ⚠️ No offline integration |
| **Rust SDK** | 10 | ❌ | ⚠️ Example only | ❌ | ❌ No real test suite |
| **Custody** | ~20+ | ❌ | ❌ | ❌ | ⚠️ Unit only |
| **Compiler** | 18 | ❌ | ❌ | ❌ | ⚠️ Unit only |
| **Deploy/Infra** | N/A | 236L JS tests | ✅ | ❌ | ✅ Good |
| **Monitoring** | N/A | 327L JS tests | ❌ | ❌ | ⚠️ OK |
| **NFT Module** | **0** | ❌ | ❌ | ❌ | ❌ **ZERO coverage** |
| **Marketplace Module** | **0** | ❌ | ❌ | ❌ | ❌ **ZERO coverage** |

---

## 5. TOP PRIORITY GAPS (Ranked by Risk)

### P0 — Critical (Blocking Production)

1. **`core/src/nft.rs` — 0 tests** for 96-line module handling NFT state
2. **`core/src/marketplace.rs` — 0 tests** for 47-line marketplace module
3. **WebSocket (`rpc/src/ws.rs`) — 1 test** for 1700+ line file handling all real-time subscriptions
4. **P2P Gossip (`p2p/src/gossip.rs`) — 0 tests** for the critical peer discovery protocol
5. **RPC DEX module (`rpc/src/dex.rs`) — 0 inline tests** for 2000+ line DEX RPC handler
6. **RPC Launchpad (`rpc/src/launchpad.rs`) — 0 tests**
7. **RPC Prediction (`rpc/src/prediction.rs`) — 0 tests**
8. **RPC DEX WebSocket (`rpc/src/dex_ws.rs`) — 0 tests**

### P1 — High (Should Fix Before Mainnet)

9. **16 contracts with only load+init coverage** (bountyboard, clawpay, clawpump, clawvault, dex_analytics, dex_rewards, dex_router, lobsterlend, moltauction, moltbridge, moltmarket, moltpunks, moltswap, moltyid, reef_storage, wbnb_token)
10. **Frontend JS tests re-implement source functions** instead of importing — tests may pass while real code is broken
11. **`caller_verification.rs` uses string matching** instead of execution-based testing
12. **Rust SDK has no real test suite** — only 10 basic inline tests + example requiring live validator
13. **Validator `main.rs` has only 2 tests** for a 12,000+ line file
14. **Missing fuzz targets:** contract WASM execution, ZK proof verification, P2P message parsing

### P2 — Medium (Technical Debt)

15. **Account, Hash, Multisig, ReefStake** — all have ≤3 tests each
16. **EVM compatibility** — only 6 tests for a complex translation layer
17. **SDK tests require live validator** — can't run in CI without infrastructure
18. **No benchmarks** for contract execution, ZK proofs, or mempool contention
19. **No chain reorg or network partition** integration tests
20. **No soak/stability tests** for memory leaks or long-running operation

---

## 6. TOTAL TEST COUNT SUMMARY

| Category | `#[test]` / Test Functions | Lines of Test Code |
|----------|---------------------------|-------------------|
| Core inline unit tests | ~274 | ~embedded in 20 source files |
| Core integration tests | ~227 | 6,816 |
| RPC integration tests | ~253 | 4,481 |
| RPC inline tests | ~25 | ~embedded |
| P2P inline tests | ~47 | ~embedded |
| Validator inline tests | ~23 | ~embedded |
| CLI inline tests | ~14 | ~embedded |
| Custody inline tests | ~20+ | ~embedded |
| Compiler inline tests | ~18 | ~embedded |
| ZK inline tests | ~36 | ~embedded |
| SDK Rust inline tests | ~10 | ~embedded |
| Fuzz targets | 8 targets | 178 |
| Rust E2E | 1 file | 577 |
| Contract test suites | 6 suites | ~130,000 (contracts/) |
| Benchmarks | 4 benchmarks | 190 |
| **Frontend JS tests** | ~500+ assertions | ~10,700 |
| **SDK JS/Python tests** | ~100+ assertions | ~906 |
| **Shell E2E tests** | ~20+ scripts | ~3,000+ |
| **GRAND TOTAL** | **~1,100+ test functions** | **~157,000+ lines** |
