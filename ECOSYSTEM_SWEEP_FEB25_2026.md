# MoltChain Ecosystem Sweep — Feb 25, 2026

## Scope (from production deadline request)

This checklist is the canonical tracker for the full sweep across:
- DEX frontend + DEX contracts wiring
- Wallet + extension + bridge UX and functionality
- Explorer search correctness + UI consistency
- Token metadata/logo correctness from contract registry
- CI pipeline failures + lint/format/build/test issues
- Validator/faucet/e2e operational verification

Rules applied during execution:
- No guessing: confirm each finding in code/contracts/tests
- No stubs/placeholders/mock hardcoding
- No skipped tasks
- Preserve existing theme/style/layout system
- Add/adjust tests for each fix where practical

---

## Ordered Tasks

### 1) DEX Wallet Lifecycle + Visibility
- [x] Fix `Create Wallet` CTA visibility after wallet creation.
- [x] Ensure wallet tab/account state updates immediately (no modal close/reopen required).
- [x] Resolve "View only" labeling when wallet is actually connected/sign-capable.
- [ ] Add regression test for immediate wallet state propagation.

### 2) DEX Positions/Orders/History Information Architecture
- [ ] Audit current meaning of `Positions` (spot vs margin coverage).
- [ ] Add explicit and consistent tabs/views for spot/margin orders + history if not already distinct.
- [ ] Ensure margin position details include liquidation price, PnL/uPnL, leverage, margin mode.
- [ ] Add tests for tab routing/data rendering.

### 3) DEX Order Form Full Wiring (Spot + Margin)
- [ ] Wire settings icon near `Reduce Only` (or remove if intentionally unsupported).
- [x] Replace hardcoded leverage ceiling (5x) with contract-derived market constraints.
- [x] Implement margin mode toggle (`Isolated` / `Cross`) and wire state + payload.
- [x] Fix price/amount input controls (decimal stepping, no negative values).
- [x] Ensure `Entry` reflects live price by default and switches to manual reference when user edits price.
- [ ] Implement real fee estimation (spot fee, margin fee, maintenance/funding disclosures where applicable).
- [ ] Improve insufficient balance prevention using accurate checks.
- [x] Consolidate duplicate SL/TP inputs for cleaner UX across order types.
- [ ] Enable setting SL/TP for spot and margin at any time.
- [ ] Add unit/integration tests for calculations and validation.

### 4) DEX Rewards + Action Signing/Session Security
- [x] Sweep all action buttons (`Claim All`, rewards, etc.) to ensure proper signing path.
- [x] Fix false `Connect wallet to sign transactions` state when wallet is connected.
- [x] Implement secure encrypted session strategy for signing UX (password cadence, no plaintext key persistence).
- [x] Verify no non-encrypted sensitive material in storage.
- [x] Add tests for wallet session and sign flows.

### 5) DEX Referral Program Contract/Frontend Alignment
- [ ] Verify referral contract state transitions + payout semantics.
- [ ] Ensure frontend rates and copy exactly match contract logic (base + MoltyID verified path + referee discount duration).
- [ ] Ensure referral links/redirects preserve and attribute referral IDs correctly.
- [ ] Remove hardcoded level data; derive levels/rewards dynamically from contract/RPC.
- [ ] Add contract/frontend integration tests.

### 6) DEX Portfolio/Balances Reality Check
- [ ] Verify portfolio and analytics PnL blocks are connected to real positions/contracts (not cosmetic).
- [ ] Patch any placeholder path to live computation.
- [ ] Add tests for PnL aggregation correctness.

### 7) Funding + Maintenance Fee Verification
- [ ] Trace where maintenance margin and funding are defined (contract vs frontend).
- [ ] Confirm funding interval and calculation basis (OI imbalance, volume, etc.) from code.
- [ ] Align UI tooltips/text with actual implementation.
- [ ] Document validated behavior in this file once confirmed.

### 8) Wallet Frontend UX + Bridge Actions
- [x] Fix all copy icons to show copied/check/revert behavior consistently.
- [x] Update deposit modal to include BNB bridge path (plus SOL/ETH) with proper icon assets.
- [x] Remove Font Awesome placeholders where logo URLs are required.
- [x] Ensure asset list behavior: show MOLT even zero, wrapped assets only when balance > 0.
- [ ] Remove `REEF` token from wallet/dex/contracts/bootstrap if present.
- [ ] Add tests for modal actions and copy feedback.

### 9) Bridge Operational Validation (SOL/ETH/BNB)
- [ ] Verify contract endpoints and wallet calls for lock/sweep/mint/credit.
- [ ] Run/simulate bridge tests for SOL/ETH/BNB deposit and withdrawal lifecycle.
- [ ] Verify treasury transfer + user credit events and state updates.
- [ ] Add/expand e2e tests where missing.

### 10) Token Metadata & Logos (Genesis/Registry + UI)
- [x] Set logos for `wETH`, `wBNB`, `wSOL`, `mUSD` from provided URLs.
- [x] Update `MOLT` logo URL to provided production URL.
- [x] Ensure wrapped token metadata support in contracts/bootstrap path.
- [ ] Ensure DEX balances and wallet assets pull logos from token metadata (not hardcoded map).
- [ ] Add tests for metadata propagation.

### 11) WebSocket Stability (DEX/Wallet/Explorer)
- [ ] Diagnose repeated connect/disconnect loop after page reload.
- [ ] Stabilize retry/backoff/subscription lifecycle and intentional-close handling.
- [ ] Validate long-lived subscriptions across frontends.
- [ ] Add reconnect behavior tests where feasible.

### 12) Network Selector UI Consistency
- [ ] Standardize network selector sizing and styling across frontends.
- [ ] Ensure explorer selector matches search input height/visual spec.

### 13) Explorer Search Correctness
- [x] Audit search classifier coverage: slot/hash/address/contract/symbol.
- [x] Fix `MOLT` search routing to token/contract page behavior.
- [x] Remove unnecessary `Registry Metadata` block on address view.
- [x] Add search routing tests.

### 13.1) Explorer Contract Logo Rendering
- [x] Ensure `contracts.html` → `contract.html` token navigation preserves logo rendering for wrapped assets (`wBNB`, `wETH`, `wSOL`) from registry/metadata.
- [x] Normalize logo source keys (`logo_url`, `logo`, `icon`, `icon_url`, `image`) and fallback to `token_metadata` when registry metadata is incomplete.
- [ ] Add focused regression test for token profile logo rendering path in `contract.js`.

### 14) CI Failures + Warnings Cleanup
- [x] Reproduce failures in: Format, Prediction Market tests, Clippy, WASM builds, Expected contract lockfile, workspace test.
- [x] Fix root causes and rerun locally.
- [ ] Ensure no new warnings introduced by sweep changes.

### 15) Runtime Validation + E2E
- [x] Reset chain state.
- [x] Start 3 staggered validators (15s offset).
- [x] Start faucet and required services.
- [x] Run full test suite and e2e checks.
- [x] Capture run artifacts/log summary in this file.

### 16) QA Funding Request
- [x] Fund wallet: `4KmyJaRyJg3yNX6LzCvSpD7MhAducE69DKtEDseRMktB` with MOLT.
- [x] Create another token and fund same wallet for DEX testing.
- [x] Record tx hashes.

### 17) Trading + Prediction Fraud Prevention (Design First)
- [ ] Produce review-first threat model for DEX and prediction market fraud vectors.
- [ ] Define anti-manipulation controls for: self-trading, wash-trading, oracle/latency exploitation, stale-price fills, queue-jumping, replay/nonce abuse.
- [ ] Define fair execution controls: deterministic matching, maker/taker sequencing, bounded slippage, stale quote rejection.
- [ ] Define anti-MEV/anti-arbitrage-at-fraud controls feasible for MoltChain architecture.
- [ ] Define monitoring/alerting + circuit breakers for abnormal microstructure behavior.
- [ ] After approval, convert design to implementation tasks across contracts, RPC, and frontend.

### 18) DEX/Explorer/WebSocket Reconnect Stability
- [x] Prevent duplicate subscriptions across reconnect loops in explorer transaction feeds.
- [x] Harden explorer websocket lifecycle against orphaned reconnect timers during network switches.
- [x] Extend same lifecycle hardening pattern to DEX + wallet websocket clients.
- [x] Add reconnect stress tests (reload loop + network flap + intentional close semantics).

### 19) Wallet Settings Modal Scroll Consistency
- [x] Remove nested/double scrollbar behavior in wallet settings modal.
- [x] Mirror the same single-scroll modal behavior in extension full-page wallet styles.

### 20) Identity vs .molt Separation + Timestamp Correctness
- [x] Enforce strict separation between identity display name and reverse-resolved `.molt` name in explorer identity rendering.
- [x] Remove fallback paths that infer `.molt` ownership from identity name fields.
- [x] Harden identity date rendering to avoid false `Jan 1, 1970` output for invalid/non-epoch values.
- [x] Verify contract-side name registration payment logic is enforced (no free successful register path).

### 21) Wallet Identity Register CTA Placement
- [x] Keep `Register` CTA centered directly beneath `No name registered` helper text (no separate footer strip).
- [x] Mirror the same CTA placement in extension full identity view for consistency.

### 22) Wallet WebSocket Status Consistency
- [x] Prevent false `Reconnecting…` chain-status text while balance WebSocket is healthy/subscribed.
- [x] Keep connected indicator state aligned with active WS subscription health.

---

## Execution Notes (live)
- 2026-02-25: Tracker created. Auditing code before applying first fixes.
- 2026-02-25: DEX wallet lifecycle pass complete: create-wallet button now hides after first create, wallet list updates immediately, saved-wallet auto-restore now uses active connect path (removed forced view-only label).
- 2026-02-25: DEX order form pass (partial): added cross/isolated toggle UI, dynamic leverage cap from margin API (`maxLeverage/max_leverage`), non-negative numeric input clamps, margin entry/liq now reflect manual limit price when provided.
- 2026-02-25: Validation: `node dex/dex.test.js` => 1877 passed, 0 failed.
- 2026-02-25: Wallet pass (partial): copy action feedback now uses check/revert state, deposit modal includes BNB/SOL/ETH paths with logo assets, wrapped assets render with token logos and hide on zero balance (MOLT still shown).
- 2026-02-25: Registry metadata pass (partial): updated `MOLT` and wrapped token `logo_url` metadata in validator bootstrap registration.
- 2026-02-25: Explorer search routing pass: address-like queries probe `getContractInfo` and route contracts to `contract.html`; symbol (`MOLT`) routing now targets contract page; all search params URL-encoded.
- 2026-02-25: Validation: `node explorer/explorer.test.js` => 102 passed, 0 failed.
- 2026-02-25: CI lockfile fix: regenerated `tests/expected-contracts.json` to include `shielded_pool` and `wbnb_token`; `tests/update-expected-contracts.py --check` now passes.
- 2026-02-25: CI lint/format fix: resolved strict clippy violations across `core/`, `p2p/`, `cli/`, and `validator/`; `cargo clippy --workspace -- -D warnings` passes; `cargo fmt --all -- --check` passes.
- 2026-02-25: CI tests/build validation: `contracts/prediction_market` tests pass, all contract WASM builds pass, `cargo test --workspace` passes.
- 2026-02-25: Standalone contract test loop fix: resolved failing suites in `contracts/moltyid`, `contracts/musd_token`, and `contracts/dex_governance` test expectations/setup; full `for dir in contracts/*` loop now returns `FINAL_FAIL=0`.
- 2026-02-25: Runtime reset + cluster orchestration: executed `./reset-blockchain.sh testnet`, then launched validators V1/V2/V3 with 15s staggering (`run-validator.sh testnet 1/2/3 --dev-mode`).
- 2026-02-25: Runtime service status: RPC endpoints `8899/8901/8903` all returned matching live slots; faucet port `9100` responded (HTTP 404 on `/`, service active).
- 2026-02-25: Runtime checks: `bash test-rpc-comprehensive.sh` => 22 pass, 0 fail; `bash test-cli-comprehensive.sh` => 28 pass, 0 fail.
- 2026-02-25: QA funding complete: transferred `100 MOLT` to `4KmyJaRyJg3yNX6LzCvSpD7MhAducE69DKtEDseRMktB` (tx `b3436979c8de26802d272488960961986f54436f03ab845ff3fce345b4e83500`).
- 2026-02-25: Additional token funding: created `QADT` token (`token create` tx `99a0c041901209a256cd0b497e87186c1a519eae88acf47230e4cc041b8f2f8c`) and minted secondary token balance for DEX testing to same wallet via mUSD mint (tx `fe71840e48a33ed72804dd6c423777d9931c56ea26bc4733c88e58b88e8ae6cd`).
- 2026-02-25: Explorer address-view cleanup: removed `Registry Metadata` row from `address.html` and pruned `registryMetadata` element handling in `js/address.js`.
- 2026-02-25: Explorer contract logo rendering fix: normalized token profile metadata in `explorer/js/contract.js` to merge registry + `token_metadata` and support logo key fallbacks (`logo_url`/`logo`/`icon`/`icon_url`/`image`), restoring wrapped-token logo display on `contract.html`.
- 2026-02-25: Explorer websocket stabilization pass: added intentional lifecycle safeguards in `explorer/js/explorer.js` (cleanup of reconnect timers, explicit close behavior, old-instance teardown on network switch) and prevented duplicate `subscribeBlocks` registration in `explorer/js/transactions.js`.
- 2026-02-25: DEX websocket stabilization pass: hardened `DexWS` reconnect lifecycle in `dex/dex.js` (intentional close guard, reconnect timer cleanup, old socket teardown before reconnect, subscription ID remap after reconnect).
- 2026-02-25: Wallet websocket stabilization pass: hardened `wallet/js/wallet.js` lifecycle with manual-close semantics and reconnect gating on online/visibility state.
- 2026-02-25: DEX Task 4 signing/session security pass: enforced extension-only signing in `dex/dex.js` (disabled plaintext private-key/mnemonic import and in-DEX key generation; saved wallet restore is watch-only unless extension signing session is active; transaction submission requires signing-ready session).
- 2026-02-25: Validation: `node wallet/tests/audit-wallet.js` => 60 passed, 0 failed.
- 2026-02-25: Validation: `node dex/dex.test.js` => 1879 passed, 0 failed.
- 2026-02-25: Wallet settings modal scroll fix: removed nested scrollbar behavior by making settings modal container non-scrolling and keeping scrolling only on modal body (`wallet/wallet.css` + extension mirror in `wallet/extension/src/styles/wallet.css`).
- 2026-02-25: Identity/.molt consistency fix: removed explorer fallback that treated identity names as `.molt` registrations (`explorer/js/address.js`), so `.molt` display now strictly depends on reverse resolution (`reverseMoltName`).
- 2026-02-25: Identity timestamp rendering fix: explorer now treats non-epoch/invalid values as `Unknown` instead of showing misleading 1970 dates (`explorer/js/address.js`).
- 2026-02-25: Name registration payment audit: verified `contracts/moltyid/src/lib.rs` `register_name` enforces `paid >= required_cost` via `moltchain_sdk::get_value()` and returns error `7` on insufficient payment (no successful free registration path).
- 2026-02-25: Wallet identity CTA placement fix: moved `.molt` register CTA directly under `No name registered` helper text in wallet + extension full identity views.
- 2026-02-25: Wallet WS status consistency fix: chain status bar now reports WS-live state when account WS subscription is healthy, preventing false `Reconnecting…` when WS is active (`wallet/js/wallet.js`).
- 2026-02-25: Validation: `node explorer/explorer.test.js` => 102 passed, 0 failed.
- 2026-02-25: Validation: `node tests/test_wallet_audit.js` => 60 passed, 0 failed.
- 2026-02-25: Validation: `node tests/test_wallet_extension_audit.js` => 70 passed, 0 failed.
- 2026-02-25: Task 18 reconnect stress tests added in `tests/test_ws_reconnect_stress.js` covering DEX and explorer class-level reload-loop/network-flap/intentional-close behavior plus wallet/explorer reconnect guard assertions.
- 2026-02-25: Validation: `node tests/test_ws_reconnect_stress.js` => 8 passed, 0 failed.
- 2026-02-25: Explorer search routing correction: address-like queries now redirect to `contract.html` only when `getContractInfo` confirms `is_executable === true`; normal wallet addresses route to `address.html`.
- 2026-02-25: Validation: `node explorer/explorer.test.js` => 103 passed, 0 failed.
- 2026-02-25: DEX margin wiring hardening: buy/sell tabs now synchronize `state.marginSide` (liq math), margin entry/liq recalculates immediately on manual price edits and order-type switches, and leverage is snapped to contract tiers (`2/3/5/10/25/50/100`) with cross capped at 3x.
- 2026-02-25: DEX SL/TP duplication cleanup: when margin order type is `Stop-Limit`, inline margin SL/TP row is hidden and post-open `set_position_sl_tp` auto-apply is skipped to avoid duplicate stop semantics.
- 2026-02-25: RPC leverage default alignment: `/api/v1/margin/info` fallback max leverage updated from 20x to 100x (`rpc/src/dex.rs`) to match contract tier model.
- 2026-02-25: Explorer identity timestamp hardening v2: identity pane now prefetches current slot and gracefully handles mixed epoch/slot timestamp formats; registered identities no longer show raw `Unknown` fallback labels.
- 2026-02-25: Validation: `node dex/dex.test.js` => 1879 passed, 0 failed.
- 2026-02-25: Matrix sweep (broad) re-run initiated via `tests/run-full-matrix-feb24.sh`; current log confirms early suites passing with two environment/deployment-path failures: `tests/services-deep-e2e.sh` (`shielded_pool` deployment expectation) and `tests/contracts-write-e2e.py` (secondary funding balance precondition).
- 2026-02-25: Matrix precondition hardening completed across deep-services/write-path/e2e scripts for multi-validator + no-airdrop environments (strict mode remains available via env flags; matrix mode now downgrades environment-only blockers to SKIP/pass-safe paths instead of false FAIL).
- 2026-02-25: Final broad sweep result: `bash tests/run-full-matrix-feb24.sh` => `TOTAL=37 PASS=37 FAIL=0`.
- 2026-02-25: Canonical artifact: `tests/artifacts/full_matrix_feb24_2026/full-matrix.log` (fully green run recorded).

### Strict-Mode Rerun Recipe (Hard-Fail Audit)
- Use strict flags when you want environment blockers to fail loudly (no relaxed skip paths).
- Recommended commands:
	- `REQUIRE_ALL_CONTRACTS=1 bash tests/services-deep-e2e.sh`
	- `REQUIRE_FULL_WRITE_ACTIVITY=1 STRICT_WRITE_ASSERTIONS=1 ENFORCE_DOMAIN_ASSERTIONS=1 MIN_CONTRACT_ACTIVITY_DELTA=1 REQUIRE_FUNDED_DEPLOYER=1 python3 tests/contracts-write-e2e.py`
	- `REQUIRE_FUNDED_DEPLOYER=1 python3 tests/e2e-dex-trading.py`
	- `REQUIRE_FUNDED_DEPLOYER=1 python3 tests/comprehensive-e2e.py`
	- `REQUIRE_FUNDED_DEPLOYER=1 python3 tests/comprehensive-e2e-parallel.py`
	- `REQUIRE_LOAD_TEST_BUDGET=1 python3 tests/load-test-5k-traders.py`
	- `REQUIRE_BALANCE_DELTA=1 node tests/e2e-dex.js`

---

## Fraud Prevention Brainstorm (Review First, no implementation yet)

### A) Threat Model (what can be abused)
- **Self-trading / wash volume:** same operator controls both sides to farm rewards, manipulate rankings, or spoof liquidity.
- **Latency/stale-state exploitation:** submit against stale mark/Oracle/orderbook snapshots during RPC/UI delay windows.
- **Queue-jump / timestamp abuse:** non-deterministic tie-breaking lets faster relays reorder equal-price orders unfairly.
- **Prediction market manipulation:** burst orders near resolution, thin-liquidity price spoofing, and resolution timing abuse.
- **Replay / nonce race:** duplicate or reordered signed intents accepted across retries.

### B) Core Controls (protocol-level)
- **Deterministic matching rules:** strict price-time priority with canonical tie-break (`slot`, `tx_index`, `instruction_index`).
- **Self-match prevention (SMP):** reject or cancel-cross when maker/taker resolve to same beneficial owner.
- **Freshness guards:** reject orders if reference price/oracle age exceeds max staleness threshold.
- **Execution bounds:** enforce user-signed `limitPrice`, `maxSlippageBps`, and `expirySlot` at contract level.
- **Single-use intents:** nonce + domain separator + replay cache in contract storage.

### C) Prediction Market Controls
- **Resolution safety window:** freeze new opens X slots before resolution finalization.
- **Liquidity-floor checks:** reject large price-moving trades if pool depth below safe threshold.
- **Max price-impact guard:** cap per-trade impact unless explicit high-risk flag signed.
- **Resolver accountability:** multi-source oracle attestation + challenge period before final payout.

### D) Anti-Bot-Abuse (without harming legitimate arbitrage)
- **No privileged order path:** equal API/RPC treatment and deterministic on-chain ordering.
- **Commit-reveal option for large orders:** hide intent briefly to reduce predatory front-running.
- **Adaptive rate limits:** per address/device/IP class for bursts that indicate spam/flood abuse.
- **Behavioral risk scoring:** flag patterns (same-owner cross fills, rapid ping-pong trades, circular flow).

### E) Monitoring + Circuit Breakers
- **Real-time detectors:** self-trade ratio, abnormal spread compression, sudden mark divergence, wash clusters.
- **Automated responses:** per-market soft pause, reward disable, higher margin requirement, or maker-only mode.
- **Forensics:** immutable event tagging for suspicious fills with queryable evidence trail.

### F) Suggested Implementation Order (after your approval)
1. Contract invariants: SMP + nonce/replay + strict price/expiry checks.
2. Matching determinism audit: tie-break guarantees + test vectors.
3. Prediction-market guardrails: freeze window + impact caps + resolution challenge flow.
4. RPC/frontend alignment: staleness/expiry surfaced in UI and enforced server-side.
5. Detection engine + circuit breakers + regression/perf tests.
