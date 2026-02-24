# MoltChain Final Pass Master TODO (Single Source of Truth)

Date: 2026-02-24  
Owner: Core team + agents  
Purpose: One canonical tracker so **nothing is forgotten**.  
Rule: Reuse existing tests first; do not duplicate unless coverage gap is explicit.

---

## 0) Non-Negotiable Acceptance Gate

- [ ] `tests/production-e2e-gate.sh` passes with strict mode (`STRICT_NO_SKIPS=1`) and **zero skipped critical workflows**.
- [ ] All required RPC + WS suites pass.
- [ ] Full user workflows verified for DEX, launchpad, prediction, wallet, explorer, marketplace, developers, identity (`.molt`), custody/multisig.
- [ ] Validator rotation analysis report completed with evidence under `400ms block` + `5s heartbeat` conditions.
- [ ] SKILL/docs alignment completed.
- [ ] Open-source boundary plan completed and enforced via repo hygiene.

---

## 1) Existing Test Inventory (Do Not Duplicate)

### 1.1 Gate / Core suites

- `tests/production-e2e-gate.sh`
- `test-rpc-comprehensive.sh`
- `test-websocket.sh`
- `test-cli-comprehensive.sh`
- `tests/live-e2e-test.sh`
- `tests/services-deep-e2e.sh`
- `tests/contracts-write-e2e.py`
- `test-contract-deployment.sh`
- `scripts/test-all-sdks.sh`

### 1.2 DEX / Launchpad / Prediction / Trading

- `tests/e2e-dex.js`
- `tests/e2e-dex-trading.py`
- `tests/e2e-launchpad.js`
- `tests/e2e-prediction.js`
- `tests/e2e-volume.js`
- `tests/test-dex-api-comprehensive.sh`
- `tests/test-ws-dex.js`

### 1.3 Wallet / Explorer / Marketplace / Developers / Web

- `tests/test_wallet_audit.js`
- `tests/test_wallet_extension_audit.js`
- `tests/test_marketplace_audit.js`
- `tests/test_developers_audit.js`
- `tests/test_website_audit.js`
- `tests/test_cross_cutting_audit.js`
- `tests/test_coverage_audit.js`

### 1.4 Genesis / Wiring / Multi-validator / Load

- `tests/e2e-genesis-wiring.py`
- `tests/multi-validator-e2e.sh`
- `tests/comprehensive-e2e.py`
- `tests/comprehensive-e2e-parallel.py`
- `tests/e2e-websocket-upgrade.py`
- `tests/load-test-5k-traders.py`
- `tests/launch-3v.sh`

### 1.5 SDK and cross-SDK

- `sdk/python/test_sdk_live.py`
- `sdk/python/test_websocket_sdk.py`
- `sdk/python/test_websocket_simple.py`
- `sdk/python/test_cross_sdk_compat.py`
- `sdk/js/test-all-features.ts`
- `sdk/js/test_cross_sdk_compat.js`
- `sdk/js/test-subscriptions.js`
- `sdk/rust/examples/test_transactions.rs`

---

## 2) Current Surgical Fixes Completed in This Pass

- [x] AMM pair duplicate prevention at contract level (canonical pair uniqueness) in `contracts/dex_amm/src/lib.rs`.
- [x] AMM adversarial duplicate test now enforces rejection (same + reversed order) in `contracts/dex_amm/tests/adversarial.rs`.
- [x] Launch tab empty-state centering fix in `dex/dex.css`.

---

## 3) Workflow Coverage Matrix (What Must Be Verified End-to-End)

## 3.1 DEX (all tabs and full lifecycle)

- Spot pair discovery and **no duplicate pair creation path** from genesis, launch graduation, or manual listing.
- Order open/modify/close, cancel-all, fees and fee accounting.
- Margin open/add/remove/close, maintenance checks, liquidation trigger path, liquidation history.
- Stop-loss auto actions and user-facing state transition consistency.
- Funding-rate/maintenance updates and realtime propagation (UI + WS).
- Rewards accrual/distribution/referrals.
- Pools/liquidity add/remove, position history.
- Governance actions affecting DEX behavior.
- Toast/notification emission for critical lifecycle events (SL, liquidation, fills, market expiry).

Status:
- Existing coverage: `tests/e2e-dex.js`, `tests/e2e-dex-trading.py`, `tests/test-dex-api-comprehensive.sh`, `tests/test-ws-dex.js`, gate suites.
- Gap tasks:
  - [ ] Add explicit DEX UI notification assertions for SL/liquidation/fill events.
  - [ ] Add explicit liquidation-history persistence checks.
  - [ ] Add WS wiring assertions per action (order, margin update, liquidation, rewards update).

## 3.2 Launchpad workflow (full)

- Token creation.
- Launch state and bonding progression.
- Upgrade/graduation flow.
- Automatic listing migration into tradable market.
- Post-graduation tradability checks in DEX.

Status:
- Existing coverage: `tests/e2e-launchpad.js`, `tests/services-deep-e2e.sh`, gate suites.
- Gap tasks:
  - [ ] Add explicit test that graduation event triggers listing visibility and tradeability in one workflow.
  - [ ] Add negative tests for duplicate listing paths after graduation.

## 3.3 Prediction market workflow (full)

- Create market.
- Bid/buy/sell participation.
- Close/finalize market.
- Credit/debit settlement and fee accounting.
- Auto actions at expiry/closing.

Status:
- Existing coverage: `tests/e2e-prediction.js`, contract tests under `contracts/prediction_market`.
- Gap tasks:
  - [ ] Add end-to-end settlement accounting assertion (wallet balances before/after + fees).
  - [ ] Add realtime WS update assertion for market lifecycle transitions.

## 3.4 Wallet + Identity + Shielding

- Wallet balances/transfers/history.
- Shield/unshield flows.
- Identity creation/update, `.molt` registration/resolution.
- Achievements + vouches lifecycle.

Status:
- Existing coverage: wallet audits + gate + contract write tests.
- Gap tasks:
  - [ ] Add explicit UI-level shielding/unshielding assertions from wallet flow.
  - [ ] Add `.molt` namespace full workflow test (register, resolve, reverse resolve, renew/release where supported).
  - [ ] Add achievement/vouch end-to-end verification from user action to explorer/wallet visibility.

## 3.5 Explorer + Developers + Marketplace

- Explorer consistency (privacy, transaction rendering, status truth mapping).
- Developers portal/API parity with current RPC/WS surface.
- Marketplace listing/trade lifecycle.

Status:
- Existing coverage: `tests/test_website_audit.js`, `tests/test_developers_audit.js`, `tests/test_marketplace_audit.js`, cross-cutting audits.
- Gap tasks:
  - [ ] Add websocket-live assertions for explorer realtime updates.
  - [ ] Add developers page endpoint parity assertions tied to `RPC_API_REFERENCE`.

## 3.6 Custody / Multisig / validator-key operations

- Multisig policy setup.
- Required-signature transfer paths.
- Operational key management for custody modules.

Status:
- Existing docs: `docs/CUSTODY_MULTISIG_SETUP.md`, `docs/deployment/CUSTODY_DEPLOYMENT.md`.
- Gap tasks:
  - [ ] Add executable e2e test for multisig transfer/approval/rejection path in CI/gate.
  - [ ] Add validator + custody key rotation scenario test.

---

## 4) Validator Rotation Deep Analysis (Required Report)

Objective: verify election spread under 400ms production blocks while heartbeat tasks run at 5s.

Required evidence:
- [ ] Slot-to-leader mapping distribution over sustained load windows.
- [ ] Per-validator block production count and variance.
- [ ] Mempool pull fairness across elected leaders.
- [ ] Correlation analysis: heartbeat timing vs leader dominance.
- [ ] Sequential vs parallel e2e impact comparison.

Execution plan:
1. Full reset.
2. Start validators one-by-one with 15s delay.
3. Run baseline no-load slot progression capture.
4. Run parallel e2e load and collect leader/slot timeline.
5. Run sequential e2e load and collect leader/slot timeline.
6. Produce `docs/audits/VALIDATOR_ROTATION_EVIDENCE_FEB24_2026.md`.

---

## 5) Final Full-Reset Revalidation Loop

Repeat until stable pass achieved N times (set N=3 minimum):

- [ ] Full reset chain state.
- [ ] Start 3 validators with 15s delay each.
- [ ] Confirm all RPC/WS endpoints healthy.
- [ ] Run strict production gate.
- [ ] Archive artifacts (`tests/artifacts/*`, logs, summaries).
- [ ] If fail: classify root cause (`test harness`, `contract logic`, `rpc/ws`, `ui wiring`) and fix surgically.

---

## 6) Questions Answered / Operational Clarifications

### 6.1 Contract write signer (`deployer.json`) in tests

- In gate tests, a privileged signer is used intentionally to validate admin/write paths deterministically.
- In production, humans/agents interact with their own keys; authorization depends on contract-level admin/caller checks.
- Genesis/admin contracts are initialized with designated admin keys; those keys should be explicitly managed in deployment secrets policy.
- If team ops access is needed, export policy must define where key material lives (secure store), who can sign, and rotation process.

Action items:
- [ ] Add explicit key management section to deployment docs (admin keys, rotation, backup, revocation).
- [ ] Add smoke test proving non-admin callers are correctly rejected on admin methods.

### 6.2 Production deployment process

- Must use documented build/deploy sequence, not ad-hoc runs.
- Required pre-mainnet pipeline: build -> deploy genesis/programs -> configure services -> run strict gate -> launch.

Action items:
- [ ] Ensure `docs/deployment/PRODUCTION_DEPLOYMENT.md` exactly matches the tested gate sequence.
- [ ] Add one command matrix table mapping local/testnet/prod procedures.

---

## 7) SKILL / Docs Alignment

- [ ] Diff current RPC/WS endpoints against `docs/guides/RPC_API_REFERENCE.md`.
- [ ] Update `skills/validator/SKILL.md` and relevant docs if any API/workflow changed.
- [ ] Verify developers docs reflect final endpoint and websocket behavior.

---

## 8) Open-Source Boundary Cleanup (Before Public Repo)

Goal: include only what is intended to be open source.

- [ ] Create explicit allowlist/denylist for repo contents.
- [ ] Move private frontends or internal-only assets to private repository/storage.
- [ ] Update `.gitignore` and repository structure accordingly.
- [ ] Run secret scan and remove sensitive files/history before publishing.
- [ ] Verify docs and scripts do not reference private paths/tokens.

Deliverable:
- [ ] `docs/deployment/OPEN_SOURCE_BOUNDARY_PLAN.md` with exact file/folder policy.

---

## 9) Immediate Next Actions (Execution Order)

1. [ ] Run full reset + 3-validator staggered boot.
2. [ ] Execute strict gate 3 consecutive passes and archive artifacts.
3. [ ] Run/extend workflow-specific gaps from sections 3.1–3.6.
4. [ ] Produce validator rotation evidence report.
5. [ ] Update SKILL/docs alignment.
6. [ ] Final open-source boundary cleanup.
7. [ ] Commit and push with clean, grouped commits.

Progress update (this session):
- [x] One fresh reset + restart completed successfully.
- [x] One strict gate run completed with `PASS:24 FAIL:0 SKIP:0`.
- [x] Validator rotation evidence report created: `docs/audits/VALIDATOR_ROTATION_EVIDENCE_FEB24_2026.md`.
- [x] Open-source boundary plan created: `docs/deployment/OPEN_SOURCE_BOUNDARY_PLAN.md`.

---

## 10) Commit Checklist (Do not push until all checked)

- [ ] Contract duplicate-pair guard merged and tested.
- [ ] UI launch empty-state centering merged.
- [ ] Strict gate green with no critical skips.
- [ ] Coverage gaps either implemented or explicitly documented as remaining blockers.
- [ ] Validator rotation report present.
- [ ] SKILL/docs aligned.
- [ ] Open-source boundary cleanup done.

---

## 11) Tracking Rules (to prevent repeated context loss)

- Every failure found must be appended to this file under its workflow section.
- Every fix must reference exact file(s) changed and test(s) run.
- No new ad-hoc TODO files; this document remains canonical.
- Keep statuses updated with `[ ]` / `[x]` only.
