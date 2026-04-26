# Mainnet Readiness Execution Tracker - 2026-04-25

This tracker records the repo-wide mainnet-readiness audit execution.

Primary plan:

- [MAINNET_READINESS_FULL_PASS_PLAN_2026-04-25.md](MAINNET_READINESS_FULL_PASS_PLAN_2026-04-25.md)

## Rules For This Audit

1. Verify every task against current source before calling it a finding.
2. Do not treat prior docs as proof without checking code or current tracked files.
3. Post the audit question, provisional answer, action, and validation before each task.
4. Do not run full workspace builds/tests by default.
5. Do not touch live VPS services or deployment state during this audit.
6. Keep changes small, scoped, and validated before moving on.
7. Record false positives explicitly so they do not come back later.

## Baseline

- Date: 2026-04-25
- Branch at plan creation: `main`
- HEAD at plan creation: `7889301 (HEAD -> main, origin/main, origin/HEAD) Contracts`
- Initial worktree: clean before creating the plan/tracker
- Active plan file created at repo root because `docs/` and `memories/` are ignored for new files.

## Part 7 - SDKs, CLI, And Developer Portal

- Time: 2026-04-26 04:25:39 +0400
- Question: do CLI, SDKs, and developer docs match current transaction format, package names, RPC surface, and public endpoint posture?
- Provisional answer: CLI/Rust SDK/JS SDK/Python SDK tests pass, but public docs are stale in package names, endpoint/version examples, dependency lists, and fixed endpoint-count claims.
- Action: patch docs only for verified drift, then rerun lightweight validation and record exact commands.
- Baseline evidence already gathered: `cargo check -p lichen-cli --tests` passed; `cargo test -p lichen-cli -- --nocapture` passed; `cd sdk/rust && cargo check --tests && cargo test -- --nocapture` passed; `cd sdk/js && npm run test && npm run build && node test_cross_sdk_compat.js` passed; `cd sdk/python && ./venv/bin/python -m pytest -q` passed with 108 passed and 3 skipped after live-validator scripts were made opt-in through `LICHEN_RUN_LIVE_SDK_TESTS=1`.
- Verified doc drift: `docs/api/RUST_SDK.md` uses `lichen_sdk` but actual crate package/library are `lichen-client-sdk` / `lichen_client_sdk`; Rust examples list nonexistent `transfer`/`query`; `docs/api/PYTHON_SDK.md` points to missing `docs/SDK.md`; JS/Python SDK docs claim "24 endpoints"; `developers/sdk-python.html` names `PyNaCl` though current dependencies use `dilithium-py`, `cryptography`, `base58`, `httpx`, and `websockets`; `docs/guides/RPC_API_REFERENCE.md` still shows version `0.4.8`, mainnet metadata samples, and a single production endpoint despite public examples using `https://testnet-rpc.lichen.network`.

## Command Ledger

| Time | Command | Result | Notes |
| --- | --- | --- | --- |
| planning | `cargo fmt --all -- --check` | pass | Low-memory formatting gate |
| planning | `python3 scripts/qa/update-expected-contracts.py --check` | pass | 28 discovered / 28 locked |
| planning | `npm run test-frontend-assets` | pass | 244 asset checks + 2 shared-helper checks |
| planning | first-party `node --check` scan | fail | `sdk/js/test_cross_sdk_compat.js` duplicate `encodeBytes` |
| Part 1.2 | `node --check sdk/js/test_cross_sdk_compat.js` | pass | After duplicate helper removal |
| Part 1.2 | `node sdk/js/test_cross_sdk_compat.js` | pass | Message vector and transaction length/hash match Rust |
| Part 1.2 | `cd sdk/js && npm run test` | pass | TypeScript `tsc --noEmit` |
| Part 1.3 | `node --check scripts/qa/audit_frontend_rpc_parity.js` | pass | New checker syntax gate |
| Part 1.3 | `npm run audit-frontend-rpc-parity` | expected fail | 8 verified unknown live frontend RPC methods |
| Part 1.4 | `git status --short --branch` | pass | Only intended Part 1 files modified/untracked |
| Part 1.4 | `node --check sdk/js/test_cross_sdk_compat.js` | pass | Focused syntax gate |
| Part 1.4 | `node --check scripts/qa/audit_frontend_rpc_parity.js` | pass | Checker syntax gate |
| Part 1.4 | `python3 scripts/qa/update-expected-contracts.py --check` | pass | 28 discovered / 28 locked |
| Part 1.4 | `node sdk/js/test_cross_sdk_compat.js` | pass | Message vector and transaction length/hash match Rust |
| Part 1.4 | `cd sdk/js && npm run test` | pass | TypeScript `tsc --noEmit` |
| Part 1.4 | `npm run test-frontend-assets` | pass | 244 asset checks + 2 shared-helper checks |
| Part 1.4 | `npm run audit-frontend-rpc-parity` | expected fail | 8 verified unknown live frontend RPC methods |
| Part 1.4 | first-party `node --check` scan | pass | Same vendor exclusions as planning sweep |
| Part 1.4 | `git diff --check` | pass | No whitespace or conflict-marker errors |
| Part 2.1 | `cargo fmt --all -- --check` | pass | Formatting gate after block-size patch |
| Part 2.1 | `cargo test -p lobstercove-lichen-core test_validate_structure_rejects_actual_pq_serialized_size_over_limit --release` | pass | Regression for actual serialized PQ-heavy block size |
| Part 2.1 | `cargo test -p lobstercove-lichen-core block::tests::test_validate_structure --release` | pass | Existing block-structure tests plus new regression |
| Part 2.2 | `cargo fmt --all -- --check` | pass | Formatting gate after transaction/RPC/mempool size patch |
| Part 2.2 | `cargo test -p lobstercove-lichen-core transaction::tests::test_validate_structure --release` | pass | Early focused run before adding from-wire regression |
| Part 2.2 | `cargo test -p lobstercove-lichen-core mempool::tests::test_mempool_rejects_invalid_transaction_before_sender_lookup --release` | pass | Early focused mempool regression |
| Part 2.2 interim | `cargo test -p lichen-rpc test_l01_rejects --release` | fail | New test referenced private items without `crate::`; test scope fixed |
| Part 2.2 | `cargo test -p lobstercove-lichen-core transaction::tests::test --release` | pass | 12 transaction tests, including size/signature/from-wire regressions |
| Part 2.2 | `cargo test -p lichen-rpc test_l01_rejects --release` | pass | 3 RPC incoming-limit tests, including oversized wire payload |
| Part 2.2 | `cargo test -p lobstercove-lichen-core mempool::tests::test_mempool --release` | pass | 9 mempool tests, including central validation regression |
| Part 2.3 | `cargo fmt --all -- --check` | pass | Formatting gate after signed-vote WAL patch |
| Part 2.3 | `cargo test -p lichen-validator wal::tests::test_signed_vote --release` | pass | WAL signed-vote recovery/checkpoint and conflict rejection |
| Part 2.3 | `cargo test -p lichen-validator consensus::tests::test_restore_signed --release` | pass | Consensus restore hooks block conflicting self-votes |
| Part 2.4 proof | `cargo test -p lichen-validator consensus::tests::test_precommit_after_commit_does_not_emit_duplicate_commit --release` | fail | Regression proved late same-height precommit emitted duplicate `CommitBlock` before patch |
| Part 2.4 | `cargo test -p lichen-validator consensus::tests::test_precommit_after_commit_does_not_emit_duplicate_commit --release` | pass | Duplicate commit regression after guard |
| Part 2.4 | `cargo fmt --all -- --check` | pass | Formatting gate after duplicate-commit guard |
| Part 2.4 | `cargo test -p lichen-validator consensus::tests::test_precommit --release` | pass | 5 precommit tests, including equivocation and duplicate-commit regression |
| Part 2.4 | `cargo test -p lichen-validator consensus::tests::test_commit_block_includes_commit_signatures --release` | pass | Commit certificate assembly still works |
| Part 2.5 | `cargo test -p lichen-validator latest_verified_checkpoint --release` | pass | 3 checkpoint exposure/root verification tests |
| Part 2.5 | `cargo test -p lichen-validator verify_checkpoint_anchor --release` | pass | 2 peer checkpoint-anchor verification tests |
| Part 2.6 | `cargo fmt --all -- --check` | pass | Formatting gate after P2P admission patch |
| Part 2.6 interim | `cargo test -p lichen-p2p p2p_admission --release` | fail | Test used core-only private fixtures; tests patched to public constructors |
| Part 2.6 | `cargo test -p lichen-p2p p2p_admission --release` | pass | 4 P2P admission helper tests |
| Part 2.6 | `cargo test -p lichen-p2p network::tests --release` | pass | 17 P2P network tests |
| Part 2.7 | `cargo fmt --all -- --check` | pass | Formatting gate after state-root fail-closed patch |
| Part 2.7 | `cargo test -p lichen-validator verify_checkpoint_anchor --release` | pass | Validator compile + 2 focused tests after mismatch branch patch |
| Part 2.8 | `cargo fmt --all -- --check` | pass | Formatting gate after block receiver/BFT progress patch |
| Part 2.8 | `cargo test -p lichen-validator verify_checkpoint_anchor --release` | pass | Validator compile + 2 focused tests after progress patch |
| Part 6.1 | `cargo test -p lichen-faucet http_support --release` | pass | 4 trusted-proxy client-IP extraction tests |
| Part 6.1 | `cargo fmt --all -- --check` | pass | Formatting gate after faucet proxy-header patch |
| Part 6.1 | `cargo test -p lichen-faucet --release -- --nocapture` | pass | 8 faucet backend tests |
| Part 6.1 | `node faucet/faucet.test.js` | pass | 43 faucet UI/source-integrity tests |
| Part 6.2 | `cargo test -p lichen-custody reuses_existing --release` | pass | 2 idempotent replay tests after replay-before-rate-limit patch |
| Part 6.2 | `cargo test -p lichen-custody policy_and_creation --release` | pass | 28 withdrawal policy/creation tests |
| Part 6.2 interim | `cargo test -p lichen-custody deposit_sweep_rebalance --release` | fail | Existing rate-limit test reused identical auth; test updated for new retry semantics |
| Part 6.2 | `cargo test -p lichen-custody deposit_sweep_rebalance --release` | pass | 45 deposit/sweep/rebalance tests |
| Part 6.2 | `cargo test -p lichen-custody --release` | pass | 110 custody tests |
| Part 6.2 | `cargo check -p lichen-custody -p lichen-faucet --tests` | pass | Combined Part 6 test compile check |
| Part 6.2 | `cargo fmt --all -- --check` | pass | Formatting gate after custody replay/rate-limit patch |
| Part 6.3 | `cargo test -p lichen-custody signing_and_assets --release` | pass | 22 signing/asset tests, including per-job EVM Safe threshold regression |
| Part 6.3 | `cargo fmt --all -- --check` | pass | Formatting gate after threshold enforcement patch |
| Part 6.3 | `cargo test -p lichen-custody --release` | pass | 111 custody tests |
| Part 6.3 | `cargo check -p lichen-custody -p lichen-faucet --tests` | pass | Combined Part 6 test compile check |
| Part 6.3 | `git diff --check` | pass | No whitespace or conflict-marker errors |
| Part 6.4 interim | `cargo test -p lichen-custody test_process_signing_withdrawals_requires_tx_intent_before_broadcast --release` | fail | Test used unqualified private module function; path fixed |
| Part 6.4 | `cargo test -p lichen-custody test_process_signing_withdrawals_requires_tx_intent_before_broadcast --release` | pass | Intent-log failure stops withdrawal before broadcast |
| Part 6.4 | `cargo fmt --all -- --check` | pass | Formatting gate after intent-log fail-closed patch |
| Part 6.4 | `cargo test -p lichen-custody --release` | pass | 112 custody tests |
| Part 6.4 | `cargo check -p lichen-custody -p lichen-faucet --tests` | pass | Combined Part 6 test compile check |
| Part 6.4 | `git diff --check` | pass | No whitespace or conflict-marker errors |
| Part 6.5 | `cargo test -p lichen-custody test_process_broadcasting_withdrawals_marks_reverted_evm_tx_failed --release` | pass | Reverted EVM withdrawal receipt becomes terminal failure |
| Part 6.5 | `cargo fmt --all -- --check` | pass | Formatting gate after confirmation patch |
| Part 6.5 | `cargo test -p lichen-custody --release` | pass | 113 custody tests |
| Part 6.5 | `cargo check -p lichen-custody -p lichen-faucet --tests` | pass | Combined Part 6 test compile check |
| Part 6.5 | `git diff --check` | pass | No whitespace or conflict-marker errors |
| Part 6.6 interim | `cargo test -p lichen-custody test_reserve_ledger_adjust_once_deduplicates_movement --release` | fail | Missing import for new reserve helper; fixed |
| Part 6.6 interim | `cargo fmt --all -- --check` | fail | New reserve test needed rustfmt wrapping; fixed |
| Part 6.6 | `cargo test -p lichen-custody test_reserve_ledger_adjust_once_deduplicates_movement --release` | pass | Reserve movement IDs deduplicate increments/debits |
| Part 6.6 | `cargo test -p lichen-custody test_process_sweep_jobs_confirmed_enqueues_credit_and_updates_status --release` | pass | Sweep confirmation still queues credit and preserves event order |
| Part 6.6 | `cargo test -p lichen-custody --release` | pass | 114 custody tests |
| Part 6.6 | `cargo fmt --all -- --check` | pass | Formatting gate after reserve idempotency patch |
| Part 6.6 | `cargo check -p lichen-custody -p lichen-faucet --tests` | pass | Combined Part 6 test compile check |
| Part 6.6 | `git diff --check` | pass | No whitespace or conflict-marker errors |
| Part 6 close | `cargo test -p lichen-faucet --release -- --nocapture` | pass | 8 faucet backend tests after custody work |
| Part 6 close | `node faucet/faucet.test.js` | pass | 43 faucet UI/source-integrity tests after custody work |
| Part 3.1 | `bash -n scripts/clean-slate-redeploy.sh scripts/vps-post-genesis.sh scripts/qa/test_local_helper_guards.sh` | pass | Syntax gate after operational script hardening |
| Part 3.1 | `bash scripts/qa/test_local_helper_guards.sh` | pass | 7 guard checks, including clean-slate confirmation refusal |
| Part 3.1 | `git diff --check` | pass | No whitespace or conflict-marker errors after script changes |
| Part 3.2 | `cargo fmt --all -- --check` | pass | Formatting gate after P2P expensive-request limiter patch |
| Part 3.2 | `cargo test -p lichen-p2p test_expensive_request_classification_includes_checkpoint_meta --release` | pass | Checkpoint metadata requests are classified as expensive |
| Part 3.2 | `cargo test -p lichen-p2p network::tests --release` | pass | 18 P2P network tests after centralized limiter |
| Part 3.3 | `cargo test -p lichen-validator load_startup_genesis_config --release` | pass | Missing genesis source and unknown network fail closed in startup config tests |
| Part 3.3 | `cargo test -p lichen-validator updater::tests --release` | pass | 13 updater tests covering signature/hash and release gating behavior |
| Part 3.3 | `cargo test -p lobstercove-lichen-core test_plaintext_keypair_load_requires_explicit_compat --release` | pass | Plaintext keypair load requires explicit compatibility opt-in |
| Part 3.3 | `cargo fmt --all -- --check` | pass | Formatting gate after plaintext keypair regression test |
| Part 3 closure | `cargo check -p lichen-validator -p lichen-p2p --tests` | pass | Scoped compile gate for Part 3 validator/P2P changes |
| Part 3 closure | `git diff --check` | pass | No whitespace or conflict-marker errors after Part 3 checks |
| Part 4.1 | `cargo test -p lichen-rpc test_m02 --release` | pass | 7 RPC rate-tier classifier tests |
| Part 4.1 | `cargo test -p lichen-rpc test_solana_get_signature_statuses_rejects_oversized_batch --release` | pass | Solana compatibility signature-status batch cap |
| Part 4.1 | `cargo fmt --all -- --check` | pass | Formatting gate after RPC rate-limit patch |
| Part 4.1 | `cargo check -p lichen-rpc --tests` | pass | Scoped compile gate for RPC crate tests |
| Part 4.1 | `git diff --check` | pass | No whitespace or conflict-marker errors after RPC patch |
| Part 4.2 | `cargo test -p lichen-rpc test_native_get_marketplace_config --release` | pass | New marketplace config compatibility RPC route |
| Part 4.2 | `npm run audit-frontend-rpc-parity` | expected fail | `getMarketplaceConfig` resolved; 7 unsafe/unimplemented frontend RPC calls remain |
| Part 4.2 | `cargo fmt --all -- --check` | pass | Formatting gate after marketplace config alias |
| Part 4.2 | `cargo check -p lichen-rpc --tests` | pass | Scoped compile gate after marketplace config alias |
| Part 4.2 | `git diff --check` | pass | No whitespace or conflict-marker errors after marketplace config alias |
| Part 4.3 | `cargo test -p lichen-rpc ws_connection_reservation_enforces_per_ip_limit_and_releases --release` | pass | WebSocket pre-upgrade reservation and release coverage |
| Part 4.3 | `cargo fmt --all -- --check` | pass | Formatting gate after WebSocket reservation patch |
| Part 4.3 | `cargo check -p lichen-rpc --tests` | pass | Scoped compile gate after WebSocket reservation patch |
| Part 4.3 | `git diff --check` | pass | No whitespace or conflict-marker errors after WebSocket reservation patch |
| Part 4 closure | `cargo test -p lichen-rpc --test rpc_full_coverage --release -- --nocapture` | pass | 230 RPC/REST compatibility coverage tests |
| Part 4 closure | `cargo test -p lichen-rpc --test shielded_handlers --release -- --nocapture` | pass | 41 shielded JSON-RPC/REST handler tests |
| Part 5.1 | `python3 scripts/qa/update-expected-contracts.py --check` | pass | 28 genesis contracts, 29 contract dirs, `mt20_token` known outside genesis |
| Part 5.1 | `python3 -m py_compile scripts/qa/update-expected-contracts.py` | pass | Syntax gate after checker hardening |
| Part 5.1 | `git diff --check` | pass | No whitespace or conflict-marker errors after contract inventory docs/checker update |
| Part 5.2 | `cd sdk && cargo test token::tests --release` | pass | 6 SDK token-standard tests, including self-transfer and failed `transfer_from` regressions |
| Part 5.2 | `cd contracts/mt20_token && cargo test --release` | pass | 8 MT-20 wrapper tests, including zero-owner/self-transfer/allowance regressions |
| Part 5.2 | `cd contracts/lusd_token && cargo test --release` | pass | 30 unit tests + 36 adversarial tests |
| Part 5.2 | `cd contracts/weth_token && cargo test --release` | pass | 14 wrapped ETH tests |
| Part 5.2 | `cd contracts/wsol_token && cargo test --release` | pass | 14 wrapped SOL tests |
| Part 5.2 | `cd contracts/wbnb_token && cargo test --release` | pass | 14 wrapped BNB tests |
| Part 5.2 | `cargo fmt --all -- --check` | pass | Root workspace formatting gate |
| Part 5.2 | `rustfmt --edition 2021 --check sdk/src/token.rs` | pass | Focused SDK token formatting gate |
| Part 5.2 | `cd contracts/mt20_token && cargo fmt -- --check` | pass | MT-20 contract formatting gate |
| Part 5.2 caveat | `cd sdk && cargo fmt -- --check` | fail | Pre-existing rustfmt drift in untouched `sdk/src/dex.rs` and `sdk/src/nft.rs`; `sdk/src/token.rs` passes focused rustfmt |
| Part 5.2 | `git diff --check` | pass | No whitespace or conflict-marker errors after token-standard patch |
| Part 5.3 interim | `cd contracts/dex_core && cargo test reduce_only --release` | pass | 5 reduce-only tests, including capped escrow regression |
| Part 5.3 interim | `cd contracts/dex_core && cargo test post_only --release` | pass | 2 unit tests + 2 adversarial post-only rejection tests |
| Part 5.3 interim | `cd contracts/dex_core && cargo test --release` | pass | 85 unit tests + 33 adversarial tests |
| Part 5.3 interim | `cd contracts/dex_core && cargo fmt -- --check` | pass | DEX Core formatting gate |
| Part 5.3 interim | `git diff --check` | pass | No whitespace or conflict-marker errors after DEX Core escrow-order patch |
| Part 5.3 dex_amm baseline | `cd contracts/dex_amm && cargo test --release` | pass | Pre-finalization AMM suite: 63 unit tests + 24 adversarial tests |
| Part 5.3 dex_amm | `cd contracts/dex_amm && cargo test first_payout_failure --release` | pass | Remove-liquidity first-payout failure keeps accounting unchanged |
| Part 5.3 dex_amm | `cd contracts/dex_amm && cargo test partial_failure --release` | pass | Partial fee-collection failure does not double-pay first token |
| Part 5.3 dex_amm | `cd contracts/dex_amm && cargo test --release` | pass | 67 unit tests + 24 adversarial tests after AMM transfer/accounting patch |
| Part 5.3 dex_amm | `cd sdk && cargo test crosscall::tests --release` | pass | 5 SDK cross-call tests after queued mock-response support |
| Part 5.3 dex_amm | `cd contracts/dex_amm && cargo fmt -- --check` | pass | DEX AMM formatting gate |
| Part 5.3 dex_amm | `rustfmt --edition 2021 --check --config skip_children=true sdk/src/lib.rs sdk/src/crosscall.rs` | pass | Focused SDK formatting gate without walking known-drift child modules |
| Part 5.3 dex_amm | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after AMM/SDK harness patch |
| Part 5.3 dex_amm | `git diff --check` | pass | No whitespace or conflict-marker errors after AMM/SDK harness patch |
| Part 5.3 dex_router baseline | `cd contracts/dex_router && cargo test --release` | pass | Pre-patch router suite: 32 unit tests |
| Part 5.3 dex_router interim | `cd contracts/dex_router && cargo test --release` | fail | Expected after behavior change; 9 tests still expected zero-output success from failed/unconfigured legs |
| Part 5.3 dex_router | `cd contracts/dex_router && cargo test --release` | pass | 32 router tests after updating failed-leg behavior and regressions |
| Part 5.3 dex_router | `cd contracts/dex_router && cargo fmt -- --check` | pass | DEX Router formatting gate |
| Part 5.3 dex_router | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after router patch |
| Part 5.3 dex_router | `git diff --check` | pass | No whitespace or conflict-marker errors after router patch |
| Part 5.3 dex_margin | `cd contracts/dex_margin && cargo test open_position_zero --release` | pass | Zero-size and zero-margin focused regressions |
| Part 5.3 dex_margin | `cd contracts/dex_margin && cargo test open_interest --release` | pass | Entry-notional open-interest decrement regressions |
| Part 5.3 dex_margin | `cd contracts/dex_margin && cargo test required_margin_uses_u128 --release` | pass | Required-margin overflow regression |
| Part 5.3 dex_margin | `cd contracts/dex_margin && cargo test --release` | pass | 113 unit tests and 28 adversarial tests after margin invariant patch |
| Part 5.3 dex_margin interim | `cd contracts/dex_margin && cargo fmt -- --check` | fail | Rustfmt wrapping drift after patch; fixed with `cargo fmt` |
| Part 5.3 dex_margin | `cd contracts/dex_margin && cargo fmt -- --check` | pass | DEX Margin formatting gate |
| Part 5.3 dex_margin | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after margin patch |
| Part 5.3 dex_margin | `git diff --check` | pass | No whitespace or conflict-marker errors after margin patch |
| Part 5.3 dex_rewards baseline | `cd contracts/dex_rewards && cargo test --release` | pass | Pre-patch rewards suite: 51 tests |
| Part 5.3 dex_rewards | `cd contracts/dex_rewards && cargo test false_transfer_status --release` | pass | Claim paths preserve rewards on `Ok(false)` transfer status |
| Part 5.3 dex_rewards | `cd contracts/dex_rewards && cargo test u128 --release` | pass | Trade/referral and LP reward math uses `u128` |
| Part 5.3 dex_rewards | `cd contracts/dex_rewards && cargo test rejects --release` | pass | Zero trade/zero liquidity and overflow rejection regressions |
| Part 5.3 dex_rewards | `cd contracts/dex_rewards && cargo test --release` | pass | 59 rewards tests after reward-accounting patch |
| Part 5.3 dex_rewards | `cd contracts/dex_rewards && cargo fmt -- --check` | pass | DEX Rewards formatting gate |
| Part 5.3 dex_rewards | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after rewards patch |
| Part 5.3 dex_rewards | `git diff --check` | pass | No whitespace or conflict-marker errors after rewards patch |
| Part 5.3 dex_governance baseline | `cd contracts/dex_governance && cargo test --release` | pass | Pre-patch governance suite: 39 tests |
| Part 5.3 dex_governance | `cd contracts/dex_governance && cargo test core_address --release` | pass | DEX core dependency setter regressions |
| Part 5.3 dex_governance | `cd contracts/dex_governance && cargo test propose_fee_change --release` | pass | Fee proposal reputation and bounds regressions |
| Part 5.3 dex_governance | `cd contracts/dex_governance && cargo test execute_new_pair --release` | pass | Stored quote and downstream status failure regressions |
| Part 5.3 dex_governance | `cd contracts/dex_core && cargo test update_fees --release` | pass | Direct DEX core maker/taker fee bound regressions |
| Part 5.3 dex_governance | `cd contracts/dex_governance && cargo test --release` | pass | 44 governance tests after proposal execution patch |
| Part 5.3 dex_governance | `cd contracts/dex_core && cargo test --release` | pass | 85 unit tests and 33 adversarial tests after maker rebate bound patch |
| Part 5.3 dex_governance | `cargo test -p lichen-genesis --release` | pass | Genesis wiring compile/test gate after governance core-address opcode |
| Part 5.3 dex_governance | `cd contracts/dex_governance && cargo fmt -- --check` | pass | DEX Governance formatting gate |
| Part 5.3 dex_governance | `cd contracts/dex_core && cargo fmt -- --check` | pass | DEX Core formatting gate after fee-bound patch |
| Part 5.3 dex_governance | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after governance patch |
| Part 5.3 dex_governance | `git diff --check` | pass | No whitespace or conflict-marker errors after governance patch |
| Part 5.3 dex_analytics baseline | `cd contracts/dex_analytics && cargo test --release` | pass | Pre-patch analytics suite: 26 tests |
| Part 5.3 dex_analytics interim | `cd contracts/dex_analytics && cargo test unauthorized --release` | pass | Filter matched 0 tests; reran with exact focused filters |
| Part 5.3 dex_analytics | `cd contracts/dex_analytics && cargo test rejects_direct --release` | pass | Direct trader/unconfigured ingestion is rejected |
| Part 5.3 dex_analytics | `cd contracts/dex_analytics && cargo test saturates --release` | pass | Analytics counters saturate instead of wrapping |
| Part 5.3 dex_analytics | `cd contracts/dex_analytics && cargo test record_pnl --release` | pass | PnL ingestion requires authorized caller and checks bounds |
| Part 5.3 dex_analytics | `cd contracts/dex_analytics && cargo test --release` | pass | 29 analytics tests after authorized-ingestion patch |
| Part 5.3 dex_analytics | `cd contracts/dex_analytics && cargo fmt -- --check` | pass | DEX Analytics formatting gate |
| Part 5.3 close | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after core DeFi family |
| Part 5.3 close | `git diff --check` | pass | No whitespace or conflict-marker errors after core DeFi family |
| Part 5.3 close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after core DeFi changes |
| Part 5.4 lichenswap baseline | `cd contracts/lichenswap && cargo test --release` | pass | Pre-patch LichenSwap suite: 32 tests |
| Part 5.4 lichenswap | `cd contracts/lichenswap && cargo test flash_loan --release` | pass | False transfer status and reserve-plus-fee overflow regressions |
| Part 5.4 lichenswap | `cd contracts/lichenswap && cargo test reputation_discount --release` | pass | Discount cap regression |
| Part 5.4 lichenswap | `cd contracts/lichenswap && cargo test --release` | pass | 34 LichenSwap tests after transfer-status and flash-loan guard patch |
| Part 5.4 lichenswap | `cd contracts/lichenswap && cargo fmt -- --check` | pass | LichenSwap formatting gate |
| Part 5.4 lichenswap | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after LichenSwap patch |
| Part 5.4 lichenswap | `git diff --check` | pass | No whitespace or conflict-marker errors after LichenSwap patch |
| Part 5.4 thalllend baseline | `cd contracts/thalllend && cargo test --release` | pass | Pre-patch ThallLend suite: 52 tests |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test overflow --release` | pass | Deposit, flash-repay, and borrow-index overflow regressions |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test oracle --release` | pass | Configured oracle failure/zero-price regressions |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test transfer_status --release` | pass | `Ok(false)` token transfer status regression |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test utilization --release` | pass | Utilization saturation regression |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test liquidation_limit --release` | pass | u128 liquidation-limit regression |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo test --release` | pass | 61 ThallLend tests after oracle/accounting patch |
| Part 5.4 thalllend | `cd contracts/thalllend && cargo fmt -- --check` | pass | ThallLend formatting gate |
| Part 5.4 thalllend | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after ThallLend patch |
| Part 5.4 thalllend | `git diff --check` | pass | No whitespace or conflict-marker errors after ThallLend patch |
| Part 5.4 prediction_market baseline | `cd contracts/prediction_market && cargo test --release` | pass | Pre-patch suite: 72 unit + 49 adversarial + 75 core + 36 resolution tests |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo test transfer_status --release` | pass | `Ok(false)` lUSD transfer status regression |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo test failed_transfer_preserves_state --release` | pass | Complete-set redemption failed payout preserves positions/pools/collateral |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo test collateral_overflow --release` | pass | Buy path rejects market collateral overflow before escrow |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo test analytics_counters --release` | pass | Trader stats and price snapshot counters saturate |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo test --release` | pass | Final suite: 76 unit + 49 adversarial + 75 core + 36 resolution tests |
| Part 5.4 prediction_market | `cd contracts/prediction_market && cargo fmt -- --check` | pass | Prediction Market formatting gate |
| Part 5.4 prediction_market | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after Prediction Market patch |
| Part 5.4 prediction_market | `git diff --check` | pass | No whitespace or conflict-marker errors after Prediction Market patch |
| Part 5.4 sporepump baseline | `cd contracts/sporepump && cargo test --release` | pass | Pre-patch SporePump suite: 47 tests |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo test false --release` | pass | `Ok(false)` LICN transfer status and sell-revert regressions |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo test create_token --release` | pass | Exact creation-fee and token-counter overflow regressions |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo test cooldown_overflow --release` | pass | Saturating cooldown deadline regression |
| Part 5.4 sporepump interim | `cd contracts/sporepump && cargo test bonding_curve_math --release` | fail | Initial expectation incorrectly required `current_price(u64::MAX)` to saturate; fixed expected finite price |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo test bonding_curve_math --release` | pass | Bonding-curve cost/refund saturation regression |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo test --release` | pass | 53 SporePump tests after transfer/accounting patch |
| Part 5.4 sporepump | `cd contracts/sporepump && cargo fmt -- --check` | pass | SporePump formatting gate |
| Part 5.4 sporepump | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after SporePump patch |
| Part 5.4 sporepump | `git diff --check` | pass | No whitespace or conflict-marker errors after SporePump patch |
| Part 5.4 markets close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after markets family |
| Part 5.5 lichenid baseline | `cd contracts/lichenid && cargo test --release` | pass | Pre-patch LichenID suite: 65 tests |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test vouch_uses_separate_given_count --release` | pass | Given-vouch indexing no longer increments received-vouch count |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test register_identity_counter_overflow --release` | pass | Identity counter overflow is rejected before identity/cooldown state writes |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test register_cooldown_overflow --release` | pass | Saturating registration cooldown deadline cannot wrap and bypass |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test false_refund_status --release` | pass | `Ok(false)` auction refund status preserves previous high bid |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test admin_register_reserved_name --release` | pass | Reserved-name admin spoofing rejected; existing reserved-name tests still pass |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test register_name_expiry_overflow --release` | pass | Name registration rejects expiry-slot overflow before state write |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test finalize_name_auction_rejects_caller_mismatch --release` | pass | Finalize caller pointer must match signer |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test refund_requires_token_config --release` | pass | Missing token config preserves previous high bid |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test refund_requires_self_address --release` | pass | Missing self-address config preserves previous high bid |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo test --release` | pass | 71 LichenID tests after identity/auction/accounting patch |
| Part 5.5 lichenid-rpc | `cargo test -p lichen-rpc test_get_lichenid_skills_with_attestations --release` | pass | RPC uses contract-compatible 16-byte FNV attestation count key |
| Part 5.5 lichenid-rpc | `cargo test -p lichen-rpc test_get_lichenid_vouches_bidirectional --release` | pass | RPC reads direct given-vouch index through `vouch_given_count` |
| Part 5.5 lichenid-rpc | `cargo test -p lichen-rpc test_get_lichenid_profile_and_directory --release` | pass | Profile/directory handler still works with new LichenID read helpers |
| Part 5.5 lichenid-rpc | `cargo check -p lichen-rpc --tests` | pass | Scoped RPC compile gate after LichenID helpers |
| Part 5.5 lichenid-core | `cargo test -p lobstercove-lichen-core achievement --release` | pass | Release compile/filter gate after saturating achievement counters; no tests matched filter |
| Part 5.5 lichenid fmt interim | `cargo fmt --all -- --check` | fail | Single rustfmt wrapping issue in new RPC helper; fixed |
| Part 5.5 lichenid | `cd contracts/lichenid && cargo fmt -- --check` | pass | LichenID formatting gate |
| Part 5.5 lichenid | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after LichenID/RPC/core patch |
| Part 5.5 lichenid | `git diff --check` | pass | No whitespace or conflict-marker errors after LichenID patch |
| Part 5.5 lichenid close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after LichenID patch |
| Part 5.5 lichendao baseline | `cd contracts/lichendao && cargo test --release` | pass | Pre-patch DAO suite: 13 tests |
| Part 5.5 lichendao | `cd contracts/lichendao && cargo test --release` | pass | 19 DAO tests after treasury-action binding and arithmetic/refund patch |
| Part 5.5 lichendao | `cd contracts/lichendao && cargo fmt -- --check` | pass | LichenDAO formatting gate |
| Part 5.5 lichendao | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after DAO patch |
| Part 5.5 lichendao | `git diff --check` | pass | No whitespace or conflict-marker errors after DAO patch |
| Part 5.5 lichendao close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after DAO patch |
| Part 5.5 lichenoracle baseline | `cd contracts/lichenoracle && cargo test --release` | pass | Pre-patch Oracle suite: 27 tests |
| Part 5.5 lichenoracle | `cd contracts/lichenoracle && cargo test --release` | pass | 33 Oracle tests after bounded input, price, attestation, and reporter-key patch |
| Part 5.5 lichenoracle | `cd contracts/lichenoracle && cargo fmt -- --check` | pass | LichenOracle formatting gate |
| Part 5.5 lichenoracle | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after Oracle patch |
| Part 5.5 lichenoracle | `git diff --check` | pass | No whitespace or conflict-marker errors after Oracle patch |
| Part 5.5 lichenoracle close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after Oracle patch |
| Part 5.6 lichenmarket baseline | `cd contracts/lichenmarket && cargo test --release` | pass | Pre-patch marketplace suite: 28 tests |
| Part 5.6 lichenmarket | `cd contracts/lichenmarket && cargo test --release` | pass | 34 marketplace tests after offer escrow, auction refund, and accounting patch |
| Part 5.6 lichenmarket | `cd contracts/lichenmarket && cargo fmt -- --check` | pass | LichenMarket formatting gate |
| Part 5.6 lichenmarket | `cargo fmt --all -- --check` | pass | Root workspace formatting gate after marketplace patch |
| Part 5.6 lichenmarket | `git diff --check` | pass | No whitespace or conflict-marker errors after marketplace patch |
| Part 5.6 lichenmarket close | `python3 scripts/qa/update-expected-contracts.py --check` | pass | Contract catalog still matches after marketplace patch |

## Part 1 - Audit Harness And Evidence Ledger

### Task 1.1 - Create Durable Tracker

Question:

- Can we establish a durable, visible tracker before touching code so every finding is tied to current-code evidence?

Provisional answer:

- Yes. A root-level tracker is required because new files under `docs/` and `memories/` are ignored in this repo.

Action:

- Create this tracker.

Status:

- Completed.

Validation:

- Confirm `git status --short --branch` shows this tracker as an untracked root file.
- Result: passed. `git status --short --branch` shows only the root plan and root tracker as untracked files.

### Task 1.2 - Verify JS SDK Golden-Vector Syntax Issue

Question:

- Is the `sdk/js/test_cross_sdk_compat.js` duplicate `encodeBytes` error a real current-code issue rather than a scanner false positive?

Provisional answer:

- Likely yes. `node --check` reported a syntax error, and readback showed one `encodeBytes` declaration at line 43 and another at line 81.

Action:

- Re-read the file, confirm the duplicate declarations are in the same scope, then remove only the duplicate if no semantic difference exists.

Planned validation:

```bash
node --check sdk/js/test_cross_sdk_compat.js
node sdk/js/test_cross_sdk_compat.js
cd sdk/js && npm run test
```

Status:

- Completed.

Follow-up evidence:

- Running `node sdk/js/test_cross_sdk_compat.js` then failed because the SDK package declares `"type": "module"` while this test used CommonJS `require()`.
- Adjacent observation: other `sdk/js/test*.js` files also use CommonJS-style `require()` under the same ESM package. This needs a separate SDK test-harness parity pass; do not treat it as part of the duplicate-helper fix without verification.

Fix:

- Removed the duplicate `encodeBytes` helper.
- Converted the test's Node imports from CommonJS `require()` to ESM `node:` imports.

Validation result:

- `node --check sdk/js/test_cross_sdk_compat.js` passed.
- `node sdk/js/test_cross_sdk_compat.js` passed.
- `cd sdk/js && npm run test` passed.

### Task 1.3 - Add RPC/Frontend Parity Checker

Question:

- Can we build a precise enough RPC/frontend parity checker now without creating noisy false positives?

Provisional answer:

- Yes for live first-party frontend JavaScript. The checker should parse the current server dispatch tables and scan tracked portal JavaScript, while classifying wallet-provider, HTTP method, WebSocket keepalive, generated/vendor, and dynamic/manual calls separately.

Action:

- Added `scripts/qa/audit_frontend_rpc_parity.js`.
- Added root npm script `audit-frontend-rpc-parity`.
- Parsed server method support from:
  - `rpc/src/lib.rs` native JSON-RPC dispatch
  - `rpc/src/lib.rs` Solana-compatible dispatch
  - `rpc/src/lib.rs` EVM-compatible dispatch
  - `rpc/src/ws.rs` WebSocket subscription dispatch
- Scanned tracked first-party frontend JS under:
  - `wallet`, `explorer`, `dex`, `marketplace`, `developers`, `programs`, `monitoring`, `faucet`, `website`

False positives ruled out during implementation:

- `LICHEN_CONFIG.rpc('mainnet')` / `LICHEN_CONFIG.rpc('testnet')` are URL resolver calls, not JSON-RPC methods.
- WebSocket `{"method":"ping"}` is handled in `rpc/src/ws.rs` as client keepalive, not a missing dispatch-table method.
- `method: 'POST'` / HTTP verbs are fetch options, not chain methods.
- Wallet-provider methods such as `licn_*` and `wallet_*` are local browser-provider APIs, not node RPC endpoints.

Validation result:

- `node --check scripts/qa/audit_frontend_rpc_parity.js` passed.
- `npm run audit-frontend-rpc-parity` intentionally exits nonzero on current code because it finds unresolved live frontend RPC methods.

Current verified unknown live frontend RPC methods:

| Method | Live call sites |
| --- | --- |
| `getMarketplaceConfig` | `marketplace/js/create.js:941` |
| `getShieldedNotes` | `wallet/extension/src/pages/full.js:1321`, `wallet/extension/src/popup/popup.js:1824` |
| `sendShieldedTransaction` | `wallet/extension/src/pages/full.js:1501` |
| `submitProgramVerification` | `programs/js/lichen-sdk.js:1415` |
| `submitShieldTransaction` | `wallet/js/shielded.js:278` |
| `submitUnshieldTransaction` | `wallet/js/shielded.js:363` |
| `submitShieldedTransfer` | `wallet/js/shielded.js:490` |

Status:

- Completed as an audit harness.
- Follow-up required: decide for each unknown method whether to add server support, change frontend wiring to supported RPC/contract calls, or mark the UI action unavailable with explicit error handling.

### Task 1.4 - Close Part 1 Focused Validation

Question:

- Should Part 1 close with the scoped validation matrix now that the SDK test and QA checker changed?

Provisional answer:

- Yes. The slice is small enough and does not require full workspace builds/tests.

Action:

- Ran only module-scoped and low-memory checks.

Validation result:

- `git status --short --branch` shows only intended Part 1 changes:
  - `package.json`
  - `sdk/js/test_cross_sdk_compat.js`
  - `MAINNET_READINESS_EXECUTION_TRACKER_2026-04-25.md`
  - `MAINNET_READINESS_FULL_PASS_PLAN_2026-04-25.md`
  - `scripts/qa/audit_frontend_rpc_parity.js`
- `node --check sdk/js/test_cross_sdk_compat.js` passed.
- `node --check scripts/qa/audit_frontend_rpc_parity.js` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.
- `node sdk/js/test_cross_sdk_compat.js` passed.
- `cd sdk/js && npm run test` passed.
- `npm run test-frontend-assets` passed.
- First-party JS/MJS `node --check` sweep passed with vendor/charting exclusions.
- `git diff --check` passed.
- `npm run audit-frontend-rpc-parity` exits nonzero by design until the 8 verified unknown frontend RPC methods are resolved.

Status:

- Completed.

## Part 2 - Core Consensus, State, And PQ Surfaces

### Task 2.1 - Block Size Must Use Actual Serialized PQ Size

Question:

- Does block structural validation enforce the actual serialized size of a block carrying self-contained ML-DSA signatures?

Provisional answer:

- It did not before this patch. The previous estimator counted each transaction signature as 64 bytes, which is incompatible with Lichen's self-contained `PqSignature` objects.

Evidence:

- `core/src/block.rs` defines `MAX_BLOCK_SIZE` as a serialized 10 MB limit.
- Native signing uses self-contained post-quantum signatures, not 64-byte signatures.
- A PQ-heavy block could pass the old estimator while exceeding the actual bincode-serialized limit.

Fix:

- `core/src/block.rs:635` now measures the block with `bincode::serialized_size(self)`.
- `core/src/block.rs:638` rejects when the actual serialized size exceeds `MAX_BLOCK_SIZE`.
- Added regression test `test_validate_structure_rejects_actual_pq_serialized_size_over_limit` at `core/src/block.rs:873`.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lobstercove-lichen-core test_validate_structure_rejects_actual_pq_serialized_size_over_limit --release` passed.
- `cargo test -p lobstercove-lichen-core block::tests::test_validate_structure --release` passed.

Status:

- Completed.

### Finding P2-01 - Signed BFT Votes Are Not Persisted Before Restart

Severity:

- Critical for mainnet readiness.

Question:

- Does the validator durably remember prevotes/precommits it has signed so a crash/restart cannot produce slashable conflicting votes for the same height, round, and vote type?

Provisional answer:

- No. Current code persists locks, commit decisions, height starts, and checkpoints, but signed prevote/precommit records are in memory only.

Evidence:

- `validator/src/consensus.rs` tracks signed votes in memory with `signed_prevote_rounds` and `signed_precommit_rounds`.
- `do_prevote` signs a prevote and then records the round in memory before returning `BroadcastPrevote`.
- `do_precommit` signs a precommit and then records the round in memory before returning `BroadcastPrecommit`.
- `validator/src/wal.rs::WalEntry` only persists `HeightStarted`, `Locked`, `CommitDecision`, and `Checkpoint`.
- `validator/src/main.rs` WAL recovery restores locked state, not prior signed prevote/precommit choices.
- `validator/src/main.rs` logs locks after consensus events, so a crash after signing/broadcasting but before durable lock persistence can lose signing history.

Risk:

- A validator that crashes after signing or broadcasting a vote can restart without durable evidence of that signed vote.
- On restart it can sign a different prevote or precommit for the same height/round/type, which is slashable equivocation under Tendermint/CometBFT-style rules.
- Persisting only locked values is not enough because equivocation protection must cover signed votes whether or not the validator has locked.

Remediation proposal:

- Add durable signed-vote records to the consensus WAL or a dedicated slashing-protection store.
- The record should include height, round, vote type, block hash or nil, timestamp for precommits, validator identity, and enough signature/signable bytes metadata to audit the decision.
- The record must be written and fsynced before the vote signature leaves the signer or is returned for broadcast.
- Startup recovery must restore this signed-vote history into the consensus engine before `start_height` can sign anything for that height.
- Add crash/restart regression tests that simulate signing a prevote/precommit, rebuilding the engine from persisted state, and attempting to sign a conflicting vote.

Status:

- Fixed in Task 2.3.

### Part 2 Resume Point

Current next investigation target:

- Transaction, RPC, P2P, and mempool byte-size controls.

Reason:

- Block inclusion now uses actual serialized block size, but mempool/RPC/P2P admission still needs verification for oversized transaction payloads, excessive PQ signatures, JSON request-body limits, and total pending-byte pressure.

### Task 2.2 - Transaction, RPC, P2P, And Mempool Size Admission

Question:

- Do transaction/RPC/P2P/mempool paths consistently reject oversized or signature-stuffed transactions before they can consume memory, CPU, or queue capacity?

Provisional answer:

- They did not. P2P had a bounded 16 MiB message envelope, but core transaction validation lacked an aggregate serialized-size cap and signature-count cap, JSON transaction decoding could bypass the bincode size limit before serde parsing, and mempool admission hashed/read sender before structural validation.

Evidence:

- `core/src/transaction.rs::validate_structure` enforced per-instruction limits but did not cap total serialized transaction size.
- `core/src/transaction.rs::validate_structure` did not cap `signatures.len()`, even though self-contained PQ signatures are large and extra signatures are ignored by required-signer verification.
- `Transaction::from_wire` documented that only bincode deserialization was bounded; JSON fell through to `serde_json::from_slice`.
- `core/src/mempool.rs::add_transaction` computed `transaction.hash()` and `transaction.sender()` before any structural validation.
- `rpc/src/lib.rs` used a 4 MiB transaction decode cap while the max deploy instruction alone is 4 MiB, and the 5 MiB HTTP body cap was too low for base64-encoded max-size transactions.
- `p2p/src/message.rs` and `p2p/src/peer.rs` already bound P2P message serialization/deserialization and encrypted stream reads to 16 MiB, so no P2P envelope patch was needed in this slice.

Fix:

- Added `MAX_SIGNATURES_PER_TX = MAX_INSTRUCTIONS_PER_TX` at `core/src/transaction.rs:146`.
- Added `MAX_TRANSACTION_SERIALIZED_SIZE = 5 * 1024 * 1024` at `core/src/transaction.rs:155`.
- `Transaction::validate_structure` now rejects excess signatures and actual bincode-serialized transactions over the cap.
- `Transaction::from_wire` now rejects oversized wire payloads before either JSON or bincode deserialization.
- `Mempool::add_transaction` now calls `transaction.validate_structure()` before hashing or sender lookup.
- RPC transaction decoding now uses the core transaction cap for wire payloads and rejects oversized wallet-JSON fallback before parsing.
- RPC central `submit_transaction` now validates incoming transaction limits before hashing or enqueueing.
- RPC HTTP request bodies now use an 8 MiB cap so base64-encoded max-size transactions can fit while still bounding request memory.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lobstercove-lichen-core transaction::tests::test --release` passed: 12 transaction tests.
- `cargo test -p lobstercove-lichen-core mempool::tests::test_mempool --release` passed: 9 mempool tests.
- `cargo test -p lichen-rpc test_l01_rejects --release` passed: 3 RPC incoming-limit tests.

Status:

- Completed.

### Part 2 Resume Point After Task 2.2

Current next investigation target:

- Signed-vote WAL/slashing-protection remediation design and implementation.

Reason:

- The remaining verified critical finding is `P2-01`: signed BFT prevotes/precommits are not durably recorded before restart. This should be resolved before broadening to less critical audit slices.

### Task 2.3 - Durable Signed-Vote WAL Slashing Protection

Question:

- Can the validator durably persist signed BFT prevotes/precommits before broadcast and restore that state after restart so it cannot sign a conflicting vote for the same height, round, and vote type?

Provisional answer:

- Yes. Persist signed-vote records in the consensus WAL before network broadcast, restore them into the consensus engine at startup, and fail closed if a vote cannot be fsynced or conflicts with an existing signed-vote record.

Fix:

- Added `SignedVoteType` and `SignedVoteRecord` to `validator/src/wal.rs`.
- Added `WalEntry::SignedVote(SignedVoteRecord)` with height, round, vote type, hash/nil, validator, signature, and precommit timestamp.
- Added fallible WAL append path for signed votes so broadcast can be skipped if persistence fails.
- Added `ConsensusWal::log_signed_prevote` and `ConsensusWal::log_signed_precommit`.
- WAL recovery now returns non-checkpointed signed-vote records and removes them after checkpoint.
- Added `ConsensusEngine::restore_signed_prevote` and `ConsensusEngine::restore_signed_precommit`.
- Restore verifies recovered signatures against the local validator and signable bytes before repopulating self-vote suppression state.
- `execute_consensus_actions` now logs local prevotes/precommits to WAL before broadcast and returns without broadcasting if persistence/conflict checks fail.
- Startup restores recovered signed-vote records after `start_height` and lock recovery.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-validator wal::tests::test_signed_vote --release` passed.
- `cargo test -p lichen-validator consensus::tests::test_restore_signed --release` passed.

Status:

- Completed.

### Task 2.4 - Prevent Duplicate CommitBlock Emission After Commit Step

Question:

- Can buffered or late same-height precommits cause the BFT engine to emit more than one `CommitBlock` action after it has already entered `RoundStep::Commit`?

Provisional answer:

- Yes. `on_precommit` checked for supermajority precommits and returned `CommitBlock` even when the engine was already in `RoundStep::Commit`; `transition_to(RoundStep::Commit)` rejected the duplicate transition, but the caller ignored that boolean.

Evidence:

- Added regression `test_precommit_after_commit_does_not_emit_duplicate_commit`.
- The regression failed before the fix: the fourth valid precommit for the same height/round/block emitted another `CommitBlock`.
- `drain_future_messages` replays all buffered precommits for a height, so this could surface during replay as well as live late-message handling.

Fix:

- `validator/src/consensus.rs:678` now returns `ConsensusAction::None` for new precommits once the engine is already in `RoundStep::Commit`.
- The guard is placed after signature/validator checks and existing-vote duplicate/equivocation handling, so known-validator equivocation evidence for already-seen precommits is still produced.
- Added regression at `validator/src/consensus.rs:2277`.

Validation result:

- Pre-fix proof run failed as expected with duplicate `CommitBlock`.
- `cargo test -p lichen-validator consensus::tests::test_precommit_after_commit_does_not_emit_duplicate_commit --release` passed after the guard.
- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-validator consensus::tests::test_precommit --release` passed: 5 tests.
- `cargo test -p lichen-validator consensus::tests::test_commit_block_includes_commit_signatures --release` passed.

Status:

- Completed.

### Task 2.5 - Checkpoint Root And Finalized Snapshot Consistency

Question:

- Does checkpoint serving expose only finalized, authenticated checkpoints, and does it correctly allow checkpoint state roots to differ from block-header roots after deterministic post-block effects?

Provisional answer:

- Yes on current code. No patch was needed.

Evidence:

- `StateStore::create_checkpoint` computes metadata `state_root` from the checkpoint store contents after the RocksDB checkpoint is opened.
- `latest_verified_checkpoint` requires `meta.slot <= state.get_last_finalized_slot()`.
- `latest_verified_checkpoint` verifies the metadata root against checkpoint contents and falls back to a cold-start root rebuild if the cached root is stale.
- `latest_verified_checkpoint` verifies the committed block at that slot with `verify_committed_block_authenticity`.
- Existing tests cover all relevant edges:
  - not exposing an unfinalized checkpoint,
  - accepting a checkpoint root that differs from the block header root after post-effects state,
  - falling back from a newer invalid checkpoint to an older valid one,
  - requiring signed committed headers for checkpoint anchors,
  - accepting checkpoint-anchor roots distinct from committed header roots.

Validation result:

- `cargo test -p lichen-validator latest_verified_checkpoint --release` passed: 3 tests.
- `cargo test -p lichen-validator verify_checkpoint_anchor --release` passed: 2 tests.

Status:

- Completed; no code changes.

### Task 2.6 - P2P Gossip And Queue Admission Bounds

Question:

- Can malformed or oversized P2P block/transaction/compact-block payloads be gossiped or queued before structural validation?

Provisional answer:

- Yes. The P2P layer had a 16 MiB deserialize envelope, but `handle_message` rebroadcast gossip messages before validating block/transaction structure, queued transactions before `Transaction::validate_structure`, and accepted compact-block / `GetBlockTxs` / `BlockTxs` vectors without protocol-level logical caps.

Evidence:

- `MessageType::Transaction` was sent to the validator transaction channel without structural validation.
- `MessageType::Block`, `BlockResponse`, and `BlockRangeResponse` were left to later receivers for structure checks, after queue admission.
- `CompactBlockMsg` could allocate/reconstruct from `short_ids.len()` even when the vector exceeded `MAX_TX_PER_BLOCK`.
- `GetBlockTxs` could carry more missing hashes than a valid block can contain.
- `BlockTxs` could carry too many transactions or structurally invalid transactions before the normal transaction receiver saw them.
- Relay/seed gossip happened before these checks, so malformed gossip could be rebroadcast.

Fix:

- Added central P2P admission helpers in `p2p/src/network.rs:57`.
- `validate_message_for_p2p_admission` now covers blocks, proposals, block responses, block range responses, transactions, compact blocks, `GetBlockTxs`, and `BlockTxs`.
- `handle_message` now rejects malformed messages and records a peer violation before gossip rebroadcast or queue admission at `p2p/src/network.rs:587`.
- Compact-block and block-tx logical vector caps are tied to `MAX_TX_PER_BLOCK`; compact-block serialized size is also capped by `MAX_BLOCK_SIZE`.
- Added focused P2P admission tests at `p2p/src/network.rs:1643`.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-p2p p2p_admission --release` passed: 4 tests.
- `cargo test -p lichen-p2p network::tests --release` passed: 17 tests.

Status:

- Completed.

### Task 2.7 - Fail Closed On Non-Genesis State-Root Mismatch

Question:

- Do BFT commit and sync/full-validation paths reject a non-default state-root mismatch before storing or applying the block?

Provisional answer:

- They did not. The code recomputed and logged diagnostics, but continued to store/apply the block even when local state did not match the block header's committed state root.

Evidence:

- BFT `CommitBlock` replay checked the root at the correct pre-effects boundary, but after a cold-root self-heal miss it only logged `STATE ROOT MISMATCH` and continued to block storage/effects.
- Sync live block application did the same for `SYNC STATE MISMATCH`.
- Pending block application did the same for `PENDING STATE MISMATCH`.
- Because transaction replay already mutates persistent state before the root check, silently continuing after mismatch can compound divergence. The safer mainnet behavior is fail-closed so operators must repair from a trusted snapshot or replay source.

Fix:

- Pending block mismatch now logs fatal context and exits before analytics, block storage, or effects at `validator/src/main.rs:8344`.
- Sync block mismatch now logs component diagnostics, fatal context, and exits before effects/storage at `validator/src/main.rs:8717`.
- BFT committed block mismatch now logs component diagnostics, fatal context, and exits before storing/effects at `validator/src/main.rs:14494`.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-validator verify_checkpoint_anchor --release` passed: 2 tests and confirmed validator crate compilation after the patch.

Status:

- Completed.

### Task 2.8 - Do Not Count Deferred Live Blocks As Sync Progress

Question:

- Does the block receiver mark sync progress for blocks it intentionally defers to BFT without applying?

Provisional answer:

- Yes. The chainable live-block deferral branch called `sync_mgr.record_progress(block_slot)` even though the block was only seen and skipped so BFT could own replay/storage.

Evidence:

- `SyncManager::record_progress` updates `last_progress_slot` and completes the active sync batch once the slot reaches the requested batch end.
- The deferral branch is for `!is_sync_block && bft_height > 0`, where the block receiver has not replayed, stored, or applied the block.
- Counting a deferred live block as progress can complete a sync batch without local chain progress.

Fix:

- Removed the false `record_progress` call from the live-block deferral branch at `validator/src/main.rs:8579`.
- Added `sync_manager.record_progress(height).await` to the real BFT `CommitBlock` path after checkpointing at `validator/src/main.rs:14652`.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-validator verify_checkpoint_anchor --release` passed: 2 tests and confirmed validator crate compilation after the patch.

Status:

- Completed.

### Part 2 Resume Point After Task 2.8

Current next investigation target:

- Continue custody/faucet/contracts/frontend audit now that signed-vote WAL protection, duplicate commit-action replay, checkpoint-root consistency, P2P admission bounds, state-root mismatch handling, and deferred-block progress accounting have been resolved or verified.

Suggested next checks:

- Broaden to custody/faucet/contracts/frontends from the main readiness plan, unless another core receiver issue appears in final review.

## Part 6 - Custody, Faucet, Bridge, And External-Chain Boundary

### Task 6.1 - Validate Faucet Trusted Proxy Client-IP Handling

Question:

- Can a configured trusted proxy path turn arbitrary forwarded header text into the faucet rate-limit key?

Provisional answer:

- Yes. The faucet only accepted `x-forwarded-for` / `x-real-ip` from peers listed in `TRUSTED_PROXY`, but once that trust branch was active it returned the first non-empty header string without parsing it as an IP address.

Evidence:

- `faucet-service/src/bootstrap.rs` enables the path from the `TRUSTED_PROXY` environment variable.
- `faucet-service/src/http_support.rs` returned trimmed header values directly.
- `request_airdrop` uses the extracted value as the per-IP rate-limit key and persists successful airdrop history with that key.
- This meant malformed or non-IP forwarded values could fragment rate limits when a trusted reverse proxy forwarded them.

Fix:

- `faucet-service/src/http_support.rs` now parses configured trusted proxies as `IpAddr` values before trusting forwarded headers.
- `x-forwarded-for` and `x-real-ip` values must parse as `IpAddr`; accepted values are normalized with `IpAddr::to_string()`.
- Malformed forwarded values fall back to the peer IP unless a valid `x-real-ip` is present from the trusted proxy path.
- Added unit tests for untrusted peers, valid forwarded headers, malformed forwarded headers, and IPv6 `x-real-ip` fallback.

Validation result:

- `cargo test -p lichen-faucet http_support --release` passed: 4 tests.
- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-faucet --release -- --nocapture` passed: 8 tests.
- `node faucet/faucet.test.js` passed: 43 tests.

Status:

- Completed.

### Part 6 Resume Point After Task 6.1

Current next investigation target:

- Continue Part 6 with custody bridge flows and deployment/trust-model docs.

Suggested next checks:

- Inspect `custody/src/*`, `docs/deployment/CUSTODY_DEPLOYMENT.md`, `docs/guides/CUSTODY_MULTISIG_SETUP.md`, and `docs/strategy/CUSTODY_ORACLE_TRUST_MODEL.md`.
- Focus first on withdrawal/deposit replay protection, signer thresholds, incident mode, stale timestamp handling, and external-chain key separation.

### Task 6.2 - Make Custody Auth Replays Truly Idempotent Under Rate Limits

Question:

- Do idempotent bridge deposit and withdrawal retries return the existing job/deposit before consuming rate-limit quota?

Provisional answer:

- No before this patch. Both handlers verified signed auth and then charged rate-limit quota before checking replay records, so a legitimate immediate retry after a client timeout could be rejected by the per-user/per-destination cooldown instead of returning the existing record.

Evidence:

- `custody/src/deposit_api_support.rs` ran deposit rate limiting before `find_existing_bridge_auth_replay`.
- `custody/src/withdrawal_api_support.rs` ran withdrawal rate limiting before `handle_withdrawal_auth_replay`.
- The existing idempotency tests had to clear `deposit_rate.per_user` / `withdrawal_rate.per_address` to pass, which masked the retry behavior.

Fix:

- Deposit creation now verifies API auth, wallet bridge auth, incident policy, and then checks replay records before charging deposit rate limits.
- Withdrawal creation now checks signed-auth replay before destination/rate-limit/new-job work.
- Existing idempotency tests now retry immediately without clearing rate-limit state and assert that quota remains charged only once.
- The deposit rate-limit HTTP test now uses a fresh valid authorization from the same user so it still verifies the cooldown path for genuinely new deposit requests.

Validation result:

- `cargo test -p lichen-custody reuses_existing --release` passed: 2 tests.
- `cargo test -p lichen-custody policy_and_creation --release` passed: 28 tests.
- Initial `cargo test -p lichen-custody deposit_sweep_rebalance --release` failed because the rate-limit test reused identical auth and now correctly received an idempotent 200 response; the test was corrected.
- `cargo test -p lichen-custody deposit_sweep_rebalance --release` passed after the test update: 45 tests.
- `cargo test -p lichen-custody --release` passed: 110 tests.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `cargo fmt --all -- --check` passed.

Status:

- Completed.

### Part 6 Resume Point After Task 6.2

Current next investigation target:

- Continue custody audit on withdrawal signer/broadcast/settlement boundaries.

Suggested next checks:

- Inspect `custody/src/withdrawal_signing_support.rs`, `custody/src/withdrawal_authorization_support/*`, `custody/src/withdrawal_broadcast_support/*`, and settlement/pending-burn transitions for threshold enforcement, stale retries, and replay-safe outbound execution.

### Task 6.3 - Enforce Per-Job Withdrawal Signer Threshold At Broadcast

Question:

- Can a persisted `signing` withdrawal bypass an elevated or extraordinary per-job signer threshold because broadcast checks only the static configured signer threshold?

Provisional answer:

- Yes before this patch. Signature collection used `effective_required_signer_threshold(job, config)`, but broadcast-time PQ approval checks and EVM Safe signature packing used `state.config.signer_threshold`. For velocity-tiered jobs, the stored `job.required_signer_threshold` can intentionally exceed the static threshold.

Evidence:

- `custody/src/withdrawal_signing_support/process.rs` collected signatures using `effective_required_signer_threshold`.
- `custody/src/withdrawal_broadcast_support.rs` checked PQ approvals against `state.config.signer_threshold`.
- `custody/src/withdrawal_broadcast_support/evm.rs` required and packed only `state.config.signer_threshold` EVM Safe signatures.
- Existing tests covered static-threshold Safe assembly but not a job with a higher stored threshold.

Fix:

- `broadcast_outbound_withdrawal` now computes `effective_required_signer_threshold(job, &state.config)` and uses it for PQ approval checks.
- `assemble_signed_evm_tx` now requires and packs the effective per-job threshold.
- Added `test_assemble_signed_evm_tx_enforces_job_required_threshold`.

Validation result:

- `cargo test -p lichen-custody signing_and_assets --release` passed: 22 tests.
- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-custody --release` passed: 111 tests.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 6 Resume Point After Task 6.3

Current next investigation target:

- Continue custody audit on outbound execution idempotency and confirmation semantics.

Suggested next checks:

- Inspect `record_tx_intent`, `clear_tx_intent`, retry handling, and broadcast/confirmation behavior for duplicate outbound transaction risks after process restart or RPC timeout.

### Task 6.4 - Fail Closed When TX Intent Logging Fails

Question:

- Do custody workers stop before broadcasting when the write-ahead transaction intent cannot be recorded?

Provisional answer:

- No before this patch. Withdrawal, sweep, and credit workers logged `record_tx_intent` failures but continued to broadcast, defeating the crash-reconciliation purpose of `CF_TX_INTENTS`.

Evidence:

- `custody/src/withdrawal_settlement_support.rs` logged `Failed record_tx_intent` and then called `broadcast_outbound_withdrawal`.
- `custody/src/sweep_execution_support.rs` did the same before `broadcast_sweep`.
- `custody/src/credit_execution_support.rs` did the same before `submit_wrapped_credit`.
- The constants document says the intent log must be written before broadcasting any on-chain transaction.

Fix:

- Withdrawal intent-log failure is now a retryable pre-broadcast failure via `mark_withdrawal_failed`, followed by `store_withdrawal_job` and `continue`.
- Sweep intent-log failure is now a retryable pre-broadcast failure via `mark_sweep_failed`.
- Credit intent-log failure is now a retryable pre-broadcast failure via `mark_credit_failed`.
- Added `test_process_signing_withdrawals_requires_tx_intent_before_broadcast`, using a test DB without `tx_intents` to prove the withdrawal worker records the failure and does not reach RPC/broadcast.

Validation result:

- Initial focused test compile failed due to an unqualified private module function path; the test was fixed.
- `cargo test -p lichen-custody test_process_signing_withdrawals_requires_tx_intent_before_broadcast --release` passed.
- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-custody --release` passed: 112 tests.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 6 Resume Point After Task 6.4

Current next investigation target:

- Continue custody audit on confirmation and settlement semantics.

Suggested next checks:

- Inspect EVM/Solana confirmation helpers and reserve-ledger updates for false confirmations, reverted transactions, missing receipt status checks, and idempotent ledger debits.

### Task 6.5 - Treat Failed External Receipts As Terminal Withdrawal Failures

Question:

- Can custody mark a failed external-chain withdrawal transaction as confirmed, or leave it stuck as a normal pending confirmation?

Provisional answer:

- Yes before this patch. EVM confirmation checked receipt depth but ignored `receipt.status`, and Solana confirmation ignored the signature status `err` field. Withdrawal confirmation also collapsed helper errors into `false`, so terminal failed receipts were not distinguished from ordinary not-yet-confirmed transactions.

Evidence:

- `custody/src/chain_confirmation_support.rs::check_evm_tx_confirmed` returned `true` based only on block depth.
- `check_solana_tx_confirmed` accepted finalized/deep statuses without checking `err`.
- `process_broadcasting_withdrawals` used `.unwrap_or(false)` on confirmation helper results.

Fix:

- Solana confirmation now returns an error when `getSignatureStatuses` reports a non-null `err`.
- EVM confirmation now requires `receipt.status` to be `0x1` or `1`; other present status values return a terminal failure error.
- Withdrawal broadcasting now treats those terminal confirmation errors as `permanently_failed`, records the error, and emits `withdrawal.permanently_failed`.
- Added `test_process_broadcasting_withdrawals_marks_reverted_evm_tx_failed`.

Validation result:

- `cargo test -p lichen-custody test_process_broadcasting_withdrawals_marks_reverted_evm_tx_failed --release` passed.
- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-custody --release` passed: 113 tests.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 6 Resume Point After Task 6.5

Current next investigation target:

- Continue custody audit on reserve-ledger idempotency and sweep/rebalance confirmation semantics.

Suggested next checks:

- Verify stablecoin reserve debits/credits are not double-applied if a confirmed withdrawal/sweep is processed again or if status-index drift replays a job.

### Task 6.6 - Make Stablecoin Reserve Movements Idempotent

Question:

- Can confirmed sweep/withdrawal processing double-apply or permanently skip reserve ledger movements across crashes or replay?

Provisional answer:

- Yes before this patch. Confirmed sweep and withdrawal processing changed job status before reserve movement. A crash between status storage and reserve update could skip accounting forever; reversing the order without idempotency could double-apply on replay.

Evidence:

- `process_sweep_jobs` stored `sweep_confirmed` before reserve increment and credit job creation.
- `process_broadcasting_withdrawals` stored `confirmed` before stablecoin reserve debit.
- Reserve ledger updates had no per-job movement marker.
- Sweep credit jobs used random UUIDs, so retry after partial processing could create duplicate queued credit jobs.

Fix:

- Added `adjust_reserve_balance_once`, which writes reserve balance updates and a per-movement marker in one RocksDB batch under the reserve lock.
- Sweep reserve increments now use movement ID `sweep:<job_id>`.
- Withdrawal reserve debits now use movement ID `withdrawal:<job_id>`.
- Stablecoin reserve movements happen before storing the final confirmed status; on retry, the movement marker prevents duplicate accounting.
- Sweep credit job IDs are now deterministic: `credit:<sweep_job_id>`, so replay overwrites the same queued credit job rather than creating a duplicate.

Validation result:

- Initial focused reserve test failed because the new helper was not imported into root scope; fixed.
- Initial formatting check failed on the new reserve test wrapping; fixed.
- `cargo test -p lichen-custody test_reserve_ledger_adjust_once_deduplicates_movement --release` passed.
- `cargo test -p lichen-custody test_process_sweep_jobs_confirmed_enqueues_credit_and_updates_status --release` passed.
- `cargo test -p lichen-custody --release` passed: 114 tests.
- `cargo fmt --all -- --check` passed.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 6 Resume Point After Task 6.6

Current next investigation target:

- Continue Part 6 with any remaining custody/faucet doc synchronization, then decide whether to move to Part 3 contracts or Part 7 SDK/CLI.

Suggested next checks:

- Reconcile custody deployment docs with the current code changes and run the full Part 6 validation slice if stopping this part.

### Part 6 Closure

Status:

- Code audit slice completed for custody/faucet bridge boundary.
- No deployment state was touched.
- Custody docs already describe the broad staged trust boundary: threshold treasury withdrawals, locally signed sweeps, and multi-signer deposit creation fail-closed. The code changes here harden retry, accounting, and confirmation semantics under that model.

Final validation:

- `cargo test -p lichen-custody --release` passed: 114 tests.
- `cargo check -p lichen-custody -p lichen-faucet --tests` passed.
- `cargo test -p lichen-faucet --release -- --nocapture` passed: 8 tests.
- `node faucet/faucet.test.js` passed: 43 tests.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

Recommended next part:

- Resume with Part 3 (`Validator, P2P, Sync, Warp Checkpoints, And Operations`) because earlier Part 2 work already touched validator/P2P/sync internals and a pass over the remaining operational edges should catch integration drift before moving to contracts/frontends.

## Part 3 - Validator, P2P, Sync, Warp Checkpoints, And Operations

### Task 3.1 - Fail Closed For Clean-Slate VPS Redeploy And Align Network Services

Question:

- Can the automated clean-slate VPS redeploy script be run accidentally or run `mainnet` with testnet custody/faucet service assumptions?

Provisional answer:

- Yes before this patch. `scripts/clean-slate-redeploy.sh` would proceed after only validating the network argument and local signing-key presence, then SSH to all configured VPSes, stop services, and delete state. Its `mainnet` mode also reused testnet custody/faucet service names, custody DB path, custody port, and testnet Cloudflare verification.

Evidence:

- `scripts/clean-slate-redeploy.sh` advertised `testnet` and `mainnet`, but had no exact operator confirmation before destructive SSH phases.
- The same script always used `lichen-custody`, `/var/lib/lichen/custody-db`, custody port `9105`, `lichen-faucet`, and `https://testnet-rpc.lichen.network` in phases that should be network-specific.
- `scripts/vps-post-genesis.sh` also created/restarted only the testnet custody DB/service and always handled faucet key material, even when `NETWORK=mainnet`.
- `deploy/setup.sh` is the current source of truth for service shape: testnet uses `lichen-custody`, `custody-db`, port `9105`, and faucet; mainnet uses `lichen-custody-mainnet`, `custody-db-mainnet`, port `9106`, and no faucet.

Fix:

- Added an exact `LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM` phrase to `scripts/clean-slate-redeploy.sh` before key checks, SSH, rsync, service stops, or deletion.
- Added network-specific custody service, custody DB, custody health port, faucet enablement, and Cloudflare RPC endpoint selection to the clean-slate script.
- Made snapshot bundling and joiner extraction conditional on faucet material only for testnet.
- Updated `scripts/vps-post-genesis.sh` to use network-specific custody DB/service selection and skip faucet provisioning/restart on mainnet.
- Extended `scripts/qa/test_local_helper_guards.sh` so CI proves clean-slate redeploy refuses to run without explicit confirmation.
- Updated the ignored local deployment runbook copy at `docs/deployment/PRODUCTION_DEPLOYMENT.md` with the confirmation phrase and mainnet note for operator continuity.

Validation result:

- `bash -n scripts/clean-slate-redeploy.sh` passed.
- `bash -n scripts/vps-post-genesis.sh` passed.
- `bash -n scripts/qa/test_local_helper_guards.sh` passed.
- `bash scripts/qa/test_local_helper_guards.sh` passed: 7 guard checks.
- `git diff --check` passed.

Status:

- Completed.

### Part 3 Resume Point After Task 3.1

Current next investigation target:

- Continue Part 3 with validator startup/sync/updater checks: unknown network handling, missing genesis behavior, plaintext keypair production guards, auto-update signature/canary discipline, and any remaining P2P request-throttling/identity gaps not already covered in Part 2.

### Task 3.2 - Rate Limit Checkpoint Metadata Requests

Question:

- Can a peer bypass expensive-request throttling by flooding `CheckpointMetaRequest` while other snapshot/status/DHT requests are rate-limited?

Provisional answer:

- Yes before this patch. Status, snapshot, state snapshot, and FindNode requests were charged against the per-peer expensive-request window, but `CheckpointMetaRequest` went straight to the snapshot request queue.

Evidence:

- `p2p/src/network.rs` rate-limited `StatusRequest`, `SnapshotRequest`, `StateSnapshotRequest`, and `FindNode` individually.
- `CheckpointMetaRequest` constructed a `SnapshotRequestMsg` with `is_meta_request: true` without checking `PeerManager::check_expensive_rate_limit`.
- Checkpoint metadata serving is part of the same sync/snapshot surface, so it should share the same per-peer budget.

Fix:

- Added a centralized `expensive_request_label` classifier for expensive request message types.
- `handle_message` now applies the per-peer expensive-request rate limit before queue admission for all classified requests, including `CheckpointMetaRequest`.
- Removed duplicated per-branch limiter checks for the previously covered request types so each request is charged exactly once.
- Added unit coverage proving `CheckpointMetaRequest` is classified as expensive while non-expensive `Ping` is not.

Validation result:

- `cargo fmt --all -- --check` passed.
- `cargo test -p lichen-p2p test_expensive_request_classification_includes_checkpoint_meta --release` passed.
- `cargo test -p lichen-p2p network::tests --release` passed: 18 tests.

Status:

- Completed.

### Part 3 Resume Point After Task 3.2

Current next investigation target:

- Continue with updater/release canary policy and validator startup false-positive verification. Current code already has unit tests for missing genesis fail-closed behavior and unknown network rejection; auto-update production defaults are off in `deploy/setup.sh`, but the runtime updater path still needs a policy pass.

### Task 3.3 - Verify Startup Genesis, Plaintext Keypair, And Auto-Update Guards

Question:

- Do validator startup, release updater, and keypair loading paths still allow production-unsafe defaults or ambiguous fallback behavior?

Provisional answer:

- Startup genesis and auto-update concerns were false positives on current code. Plaintext keypair load policy was already fail-closed for normal production paths, and this task added focused regression coverage so the compatibility escape hatch remains explicit.

Evidence:

- `validator/src/main.rs` startup configuration requires an explicit genesis source unless local dev mode is selected, and rejects unknown network names.
- `validator/src/updater.rs` verifies `SHA256SUMS.sig` against the embedded release-signing address and checks the downloaded archive hash before installing.
- `.github/workflows/release.yml` creates draft releases and includes manual detached-signature/canary instructions before publishing.
- `deploy/setup.sh` writes `LICHEN_EXTRA_ARGS=--auto-update=off` by default, so updater activation is explicit in deployed service args.
- `core/src/keypair_file.rs` rejected plaintext private-key material unless the caller opted into plaintext compatibility, but there was no tight regression test for the loader-level policy.

Fix:

- Added `test_plaintext_keypair_load_requires_explicit_compat` in `core/src/keypair_file.rs`.
- The test writes a legacy plaintext keypair file, proves normal loading rejects it with a message pointing to `ALLOW_PLAINTEXT_KEYPAIR_ENV`, and proves explicit compatibility loading still works for migration flows.
- No code changes were needed for startup genesis selection or updater release policy after reviewing current implementation and tests.

Validation result:

- `cargo test -p lichen-validator load_startup_genesis_config --release` passed: 4 startup config tests.
- `cargo test -p lichen-validator updater::tests --release` passed: 13 updater tests.
- `cargo test -p lobstercove-lichen-core test_plaintext_keypair_load_requires_explicit_compat --release` passed.
- `cargo fmt --all -- --check` passed.
- `cargo check -p lichen-validator -p lichen-p2p --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 3 Closure

Status:

- Validator/P2P/operations audit slice is complete for the scoped mainnet-readiness pass.
- No live deployment state was touched.
- Operational scripts now fail closed for destructive redeploys and use network-specific service assumptions.
- P2P expensive request throttling now covers checkpoint metadata.
- Startup genesis selection, auto-update release verification/defaults, and plaintext keypair handling have focused validation coverage or reviewed existing fail-closed behavior.

Recommended next part:

- Resume with Part 4 RPC/REST/WebSocket compatibility and rate-limit review.

## Part 4 - RPC, REST, WebSocket, Compatibility APIs, And Rate Limits

### Task 4.1 - Tighten Public RPC Rate-Limit Tier Coverage

Question:

- Do public RPC/REST compatibility methods that perform proof generation, external custody calls, index scans, or full block serialization fall through to cheap per-IP limits?

Provisional answer:

- Yes before this patch. The tiering infrastructure existed, but several native, Solana-compatible, EVM-compatible, and REST surfaces defaulted to cheap limits despite doing CPU-heavy proof generation, custody HTTP calls, bounded but non-trivial scans, or full object serialization.

Evidence:

- Native JSON-RPC `generateShieldProof`, `generateUnshieldProof`, and `generateTransferProof` build and self-verify STARK proofs but were not listed as expensive.
- Native `createBridgeDeposit` verifies wallet-signed bridge access and proxies to custody `POST /deposits`, but was cheap-tier.
- Native reads such as `getTransaction`, `getTransactionProof`, `getLatestBlock`, marketplace offer/auction queries, shielded commitments/merkle path, prediction scans, bridge deposit lookup, DEX analytics, and DEX pairs were cheap-tier.
- Solana-compatible `getTransaction` and `getTokenAccountsByOwner` were cheap-tier despite fallback scans or bounded account iteration.
- EVM-compatible `eth_getBlockByNumber`, `eth_getBlockByHash`, `eth_getCode`, and `eth_getTransactionCount` were cheap-tier; only `eth_getLogs` had moderate coverage.
- REST scan routes such as `/api/v1/launchpad/tokens`, `/api/v1/shielded/commitments`, `/api/v1/shielded/merkle-path/:index`, `/api/v1/routes`, `/api/v1/pools`, and `/api/v1/tickers` did not match the moderate path classifier.
- Solana `getSignatureStatuses` iterated the caller-provided signature array without a count cap inside the 8 MiB JSON-RPC body limit.

Fix:

- Expanded native `classify_method` so proof generation and bridge deposit creation are expensive-tier.
- Expanded native moderate-tier coverage for scan/proof/external-lookup methods.
- Added `classify_evm_method_tier` and applied it in EVM dispatch.
- Expanded Solana moderate-tier coverage for transaction and token-account lookups.
- Expanded REST moderate path matching for launchpad token scans, shielded commitment/proof reads, and DEX route/pool/ticker scans.
- Added a 256-signature cap to Solana-compatible `getSignatureStatuses`.
- Added focused classifier and oversized-batch regression tests.

Validation result:

- `cargo test -p lichen-rpc test_m02 --release` passed: 7 tests.
- `cargo test -p lichen-rpc test_solana_get_signature_statuses_rejects_oversized_batch --release` passed.
- `cargo fmt --all -- --check` passed.
- `cargo check -p lichen-rpc --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 4 Resume Point After Task 4.1

Current next investigation target:

- Continue Part 4 with the existing frontend/RPC parity gap: `getMarketplaceConfig`, shielded-note/submission methods, and `submitProgramVerification` are called by checked-in frontends or SDK helpers but are not implemented by the RPC dispatch. Determine which are safe server aliases and which need frontend/client contract changes.

### Task 4.2 - Add Marketplace Config Compatibility RPC And Classify Remaining Parity Gaps

Question:

- Can any frontend/RPC parity gaps be safely closed at the RPC layer without inventing unsigned transaction semantics?

Provisional answer:

- Yes for `getMarketplaceConfig`; no for the current shielded note/submission and program verification calls without broader frontend/client contract changes.

Evidence:

- `marketplace/js/create.js` calls `getMarketplaceConfig` only to read `minting_fee`.
- The authoritative NFT mint fee already lives in `FeeConfig` as `nft_mint_fee`, exposed by `getFeeConfig` as `nft_mint_fee_spores`.
- The marketplace WASM contract stores `marketplace_fee` as bps, with default 250 bps. That can be returned as advisory marketplace config when the `MARKET` symbol exists, with the same default otherwise.
- `wallet/js/shielded.js` calls `submitShieldTransaction`, `submitUnshieldTransaction`, and `submitShieldedTransfer` with proof fragments and encrypted-note fields, while the existing RPC/REST submission path intentionally accepts only signed transaction payloads.
- The wallet extension calls `sendShieldedTransaction` with wallet password material, which should not be converted into an RPC server-side signing path.
- `getShieldedNotes` is called with only a shielded address, but note ownership requires local viewing-key trial decryption or another explicit viewing-key contract; the server cannot infer private note ownership from address alone.
- `programs/js/lichen-sdk.js` falls back to local queued verification when `submitProgramVerification` is unavailable; no server-side verification queue exists in the RPC crate.

Fix:

- Added native JSON-RPC `getMarketplaceConfig`.
- The response includes `minting_fee`, `minting_fee_spores`, `nft_mint_fee_spores`, `nft_collection_fee_spores`, `marketplace_fee_bps`, `marketplace_fee_percent`, and `marketplace_configured`.
- Added integration coverage for the new route.
- Re-ran the frontend/RPC parity checker: `getMarketplaceConfig` is no longer unknown; 7 remaining calls are still flagged and should be fixed by frontend/client contract work rather than RPC aliases.

Validation result:

- `cargo test -p lichen-rpc test_native_get_marketplace_config --release` passed.
- `npm run audit-frontend-rpc-parity` expected-failed with 7 unknown call sites across 6 methods: `getShieldedNotes`, `sendShieldedTransaction`, `submitProgramVerification`, `submitShieldedTransfer`, `submitShieldTransaction`, and `submitUnshieldTransaction`.
- `cargo fmt --all -- --check` passed.
- `cargo check -p lichen-rpc --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 4 Resume Point After Task 4.2

Current next investigation target:

- Continue Part 4 with WebSocket connection/message limits and public error/CORS review, or move the remaining shielded/program verification parity gaps into the frontend/SDK part because they require client contract changes rather than safe RPC aliases.

### Task 4.3 - Reserve WebSocket Connection Slots Before Upgrade

Question:

- Can concurrent WebSocket handshakes bypass global or per-IP connection limits because counters are incremented only after the upgrade task starts?

Provisional answer:

- Yes before this patch. `ws_handler` checked `active_connections` and `IP_CONNECTIONS`, then accepted the upgrade; `handle_socket` incremented both counters later. A burst of simultaneous handshakes could all observe capacity before any upgraded socket incremented the counters.

Evidence:

- `ws_handler` read the global and per-IP counters but did not reserve a slot.
- `handle_socket` incremented the counters after the upgrade was accepted.
- Axum 0.7.9 exposes `WebSocketUpgrade::on_failed_upgrade`, so slots can be reserved before `on_upgrade` and released if upgrade fails.

Fix:

- Added `try_reserve_ws_connection`, which atomically reserves the global connection slot with compare-and-swap and reserves the per-IP slot under the existing mutex.
- Added `release_ws_connection`, using saturating atomic release plus existing per-IP cleanup.
- `ws_handler` now reserves before accepting the upgrade, releases on failed upgrade, and `handle_socket` releases on disconnect.
- Removed the post-upgrade counter increments from `handle_socket`.
- Added regression coverage for same-IP limit enforcement and release cleanup.

Validation result:

- `cargo test -p lichen-rpc ws_connection_reservation_enforces_per_ip_limit_and_releases --release` passed.
- `cargo fmt --all -- --check` passed.
- `cargo check -p lichen-rpc --tests` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 4 Resume Point After Task 4.3

Current next investigation target:

- Finish Part 4 with a short public CORS/error-response review and a scoped final RPC validation slice.

### Task 4.4 - Public CORS And Error-Response Review

Question:

- Do public CORS and JSON-RPC error responses expose unsafe defaults or sensitive internals after the Part 4 changes?

Provisional answer:

- No code change needed in this pass. The current CORS and error-response paths already enforce the intended public boundary.

Evidence:

- `build_rpc_router` defaults CORS to explicit localhost and `lichen.network` portal hosts.
- Mainnet startup exits if `LICHEN_CORS_ORIGINS` contains wildcard `*`.
- CORS origin matching parses `http://` / `https://` origins and compares exact hosts, preventing suffix-style host bypass.
- CORS preflight is allowed through rate limiting, but only the configured CORS middleware decides allowed origins.
- JSON-RPC parse/invalid-request helpers return standard `-32700` / `-32600` JSON-RPC envelopes.
- Dispatch responses pass handler errors through `sanitize_rpc_error`, which collapses storage internals to `Database error` and redacts local filesystem paths.
- Legacy admin RPCs remain disabled outside local/dev exact network IDs, strip any body `admin_token`, require `Authorization: Bearer`, and require loopback clients in local/dev mode. Existing integration tests cover public-network disablement, dev bearer auth, and dev loopback rejection.

Fix:

- No code changes.

Validation result:

- `cargo test -p lichen-rpc --test rpc_full_coverage --release -- --nocapture` passed: 230 tests.
- `cargo test -p lichen-rpc --test shielded_handlers --release -- --nocapture` passed: 41 tests.

Status:

- Completed.

### Part 4 Closure

Status:

- RPC/REST/WebSocket compatibility and rate-limit audit slice is complete for this pass.
- No live deployment state was touched.
- Remaining frontend/RPC parity gaps are intentionally deferred to frontend/SDK work because the current frontend payloads are incompatible with safe server-side RPC aliases.

Recommended next part:

- Resume with Part 5 contracts and genesis catalog.

## Part 5 - Contracts And Genesis Catalog

### Task 5.1 - Make `mt20_token` Non-Genesis Status Explicit

Question:

- Is the 29-directory / 28-genesis-contract state intentional and enforced clearly enough for release audits?

Provisional answer:

- It was intentional but under-enforced. `mt20_token` existed in-tree and was excluded from `GENESIS_CONTRACT_CATALOG`, but the expected-contracts checker only derived names from genesis and did not inspect `contracts/` for unexpected non-genesis directories. Public contract docs also did not explain the exclusion.

Evidence:

- `find contracts -mindepth 2 -maxdepth 2 -name Cargo.toml` lists 29 contract directories.
- `GENESIS_CONTRACT_CATALOG` contains 28 contracts and excludes `mt20_token`.
- `scripts/qa/update-expected-contracts.py --check` previously reported only 28 discovered contracts because it discovered from genesis, not from the filesystem.
- `developers/contract-reference.html` did not mention `mt20_token` in the authoritative export matrix.

Fix:

- Hardened `scripts/qa/update-expected-contracts.py` to also count contract directories, detect catalog entries missing directories, and fail `--check` on unexpected in-tree contracts outside genesis.
- Added `KNOWN_NON_GENESIS_CONTRACTS = {"mt20_token"}` so the current exclusion is explicit rather than implicit.
- Updated `developers/contract-reference.html` to state that `mt20_token` remains an in-tree standalone MT20 template/compatibility contract and is intentionally not deployed by `GENESIS_CONTRACT_CATALOG`.

Validation result:

- `python3 scripts/qa/update-expected-contracts.py --check` passed and reported 28 discovered genesis contracts, 28 lockfile contracts, 29 contract directories, and `mt20_token (known)` outside genesis.
- `python3 -m py_compile scripts/qa/update-expected-contracts.py` passed.
- `git diff --check` passed.

Status:

- Completed.

### Part 5 Resume Point After Task 5.1

Current next investigation target:

- Continue Part 5 with the first contract family: tokens and wrapped assets (`lusd_token`, `weth_token`, `wsol_token`, `wbnb_token`, and non-genesis `mt20_token`).

### Task 5.2 - Token And Wrapped-Asset Contract Family

Question:

- Do the token and wrapped-asset contracts preserve supply and allowance invariants under adversarial token-standard calls while keeping reserve-backed wrapped assets guarded by admin/minter/attester separation, pause controls, epoch caps, and attestation circuit breakers?

Provisional answer:

- The wrapped assets already carried the relevant current-code guards and passed their focused test slices. The standalone MT-20 template had a real shared-token-standard bug: `Token::transfer` did not reject `from == to`, so a self-transfer calculated debit and credit from the same original balance and then wrote the same balance key twice, inflating the account by `amount`. `Token::transfer_from` also persisted the reduced allowance before proving that the transfer could succeed, so a failed balance/overflow/invalid transfer could still consume allowance.

Evidence:

- `sdk/src/token.rs::transfer` computed `new_from` and `new_to` from the original balances and then called `set_balance(from, new_from)` followed by `set_balance(to, new_to)`.
- When `from == to`, both writes target the same key and the final write stores `balance + amount`.
- `sdk/src/token.rs::transfer_from` wrote the decremented allowance before calling `self.transfer(from, to, amount)`.
- `contracts/mt20_token/src/lib.rs` used the shared `Token` helper and did not independently reject zero owner initialization or self-transfers.
- `contracts/lusd_token`, `contracts/weth_token`, `contracts/wsol_token`, and `contracts/wbnb_token` already reject self-transfer, zero transfer amounts, zero recipients, caller mismatches, and checked arithmetic before state writes in the covered paths.

Fix:

- Added shared MT-20 address/input validation in `sdk/src/token.rs`.
- `Token::transfer` now rejects zero amounts, zero-address endpoints, and self-transfer before computing balance deltas.
- `Token::mint`, `Token::burn`, and `Token::approve` now reject invalid zero/self-address inputs consistently with the wrapped-token contracts.
- `Token::transfer_from` now validates caller/self-transfer inputs and only persists the decremented allowance after `self.transfer` succeeds.
- `contracts/mt20_token/src/lib.rs` now rejects zero owner initialization before storing owner state.
- Added SDK regressions for self-transfer inflation and failed `transfer_from` allowance preservation.
- Added MT-20 wrapper regressions for zero owner initialization, self-transfer inflation, and failed `transfer_from` allowance preservation.

Validation result:

- `cd sdk && cargo test token::tests --release` passed: 6 token-standard tests.
- `cd contracts/mt20_token && cargo test --release` passed: 8 MT-20 wrapper tests.
- `cd contracts/lusd_token && cargo test --release` passed: 30 unit tests and 36 adversarial tests.
- `cd contracts/weth_token && cargo test --release` passed: 14 tests.
- `cd contracts/wsol_token && cargo test --release` passed: 14 tests.
- `cd contracts/wbnb_token && cargo test --release` passed: 14 tests.
- `cargo fmt --all -- --check` passed for the root workspace.
- `rustfmt --edition 2021 --check sdk/src/token.rs` passed.
- `cd contracts/mt20_token && cargo fmt -- --check` passed.
- `git diff --check` passed.
- `cd sdk && cargo fmt -- --check` failed on pre-existing rustfmt drift in untouched `sdk/src/dex.rs` and `sdk/src/nft.rs`; this patch's `sdk/src/token.rs` passes focused rustfmt.

Status:

- Completed.

### Part 5 Resume Point After Task 5.2

Current next investigation target:

- Continue Part 5 with the core DeFi contract family: `dex_core`, `dex_amm`, `dex_router`, `dex_margin`, `dex_rewards`, `dex_governance`, and `dex_analytics`.

### Task 5.3 - Core DeFi Contract Family

Question:

- Do the core DeFi contracts reject invalid orders and governance/admin paths before irreversible token movement, and do escrow/accounting fields stay consistent with the final accepted order?

Provisional answer:

- `dex_core` had a confirmed escrow-ordering bug in `place_order`. The remaining core DeFi contracts still need their current-code pass in this slice.

Evidence:

- `contracts/dex_core/src/lib.rs::place_order` escrowed base/quote tokens before post-only crossing checks. A post-only order that would immediately cross returned code `7` after escrow had already succeeded.
- The same function escrowed tokens before reduce-only margin validation. A reduce-only order with no margin address, no open position, or wrong closing side returned code `12` after escrow could already have succeeded.
- Reduce-only quantity capping also happened after escrow. A reduce-only order larger than the open position could lock the requested quantity while creating an order for the capped position size.

Fix:

- Moved post-only would-cross rejection before token escrow.
- Moved reduce-only margin validation before token escrow.
- Moved notional/minimum-order validation after reduce-only capping so it evaluates the actual accepted order size.
- Escrow for reduce-only orders is now calculated from the capped quantity.
- Added regression `test_reduce_only_cap_applies_before_escrow`.

Validation result:

- `cd contracts/dex_core && cargo test reduce_only --release` passed: 5 tests.
- `cd contracts/dex_core && cargo test post_only --release` passed: 2 unit tests and 2 adversarial tests.
- `cd contracts/dex_core && cargo test --release` passed: 85 unit tests and 33 adversarial tests.
- `cd contracts/dex_core && cargo fmt -- --check` passed.
- `git diff --check` passed.

DEX AMM follow-up question:

- Do AMM liquidity, swap, fee-collection, and protocol-fee flows preserve user/accounting claims when token/native transfers fail, including partial two-token payout failures?

DEX AMM provisional answer:

- No. The current code had several unchecked or partially checked transfer paths. Some were already patched in the interrupted pre-resume worktree, but re-review found more partial-payment accounting hazards.

DEX AMM evidence:

- `send_tokens` returned unconditional success in non-WASM tests, so AMM unit tests could not prove outbound transfer failure handling.
- `add_liquidity_with_deadline` pulled token A before token B; if token B failed, token A needed an explicit refund attempt before returning failure.
- `swap_exact_in` pulled input before output transfer; if output transfer failed, input needed a refund attempt before returning failure.
- `collect_fees` could pay token A and then fail on token B without updating position accounting, allowing token A to be paid again on retry.
- `remove_liquidity_with_deadline` stored reduced position/pool/tick accounting before outbound payouts and ignored send results, so a transfer failure could burn liquidity without delivering tokens.
- `collect_protocol_fees` ignored outbound transfer failures and could clear accrued protocol fees even if the treasury transfer failed.

DEX AMM fix:

- Unified `send_tokens` across tests and production so native/token transfer failures are observable in host regressions.
- `add_liquidity_with_deadline` now attempts to refund token A if the token B pull fails.
- `swap_exact_in` now attempts to refund input if output transfer fails.
- `collect_fees` now keeps fees owed if the first transfer fails; if token A succeeds and token B fails, it zeroes paid token A, retains unpaid token B, updates the fee-growth snapshot, and returns transfer failure.
- `remove_liquidity_with_deadline` now returns code `6` without mutating liquidity accounting if no payout succeeded. If token A has already been paid and token B fails, it records unpaid token B into the position's owed balance, applies the liquidity/accounting reduction, and returns code `6` so the partial payout is visible without enabling double withdrawal.
- `collect_protocol_fees` now returns transfer failure instead of silently clearing unpaid accrued protocol fees.
- Added queued mock cross-call responses in `sdk/src/lib.rs` / `sdk/src/crosscall.rs` so tests can model first-transfer success followed by second-transfer failure.

DEX AMM validation result:

- Baseline `cd contracts/dex_amm && cargo test --release` passed before finalizing the patch: 63 unit tests and 24 adversarial tests.
- `cd contracts/dex_amm && cargo test first_payout_failure --release` passed.
- `cd contracts/dex_amm && cargo test partial_failure --release` passed.
- `cd contracts/dex_amm && cargo test --release` passed after the patch: 67 unit tests and 24 adversarial tests.
- `cd sdk && cargo test crosscall::tests --release` passed: 5 tests.
- `cd contracts/dex_amm && cargo fmt -- --check` passed.
- `rustfmt --edition 2021 --check --config skip_children=true sdk/src/lib.rs sdk/src/crosscall.rs` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- Caveat: direct `rustfmt --edition 2021 --check sdk/src/lib.rs sdk/src/crosscall.rs` still walks child modules and reports the known pre-existing rustfmt drift in untouched `sdk/src/dex.rs` and `sdk/src/nft.rs`; the root workspace formatting gate and focused changed-file gate both passed.

DEX Router follow-up question:

- Does `dex_router` distinguish a failed cross-contract route leg from a successful swap that returns an amount, especially when callers set `min_out=0`?

DEX Router provisional answer:

- No. Current code collapsed failed/unconfigured route legs into `amount_out = 0`, then recorded a successful routed swap whenever the caller allowed zero minimum output.

DEX Router evidence:

- `execute_clob_swap` and `execute_amm_swap` returned `0` both for cross-contract call failure and for a real returned amount.
- `swap` accepted that `0` as success when `min_amount_out == 0`, wrote a swap record, incremented `SWAP_COUNT_KEY`, and added to routed volume.
- Split and registered multi-hop routes similarly treated zero-output failed legs as successful route execution when the final minimum was zero.
- Baseline tests encoded the problem by expecting `test_swap_no_simulation_fallback` to succeed with `amount_out=0`.

DEX Router fix:

- `execute_clob_swap` and `execute_amm_swap` now return `Option<u64>`, using `None` for unconfigured/malformed/failed cross-contract calls.
- Direct, split, registered multi-hop, and explicit `multi_hop_swap` paths now return code `7` for failed or zero-output legs instead of recording a successful zero-output swap.
- Split routes now reject zero-sized legs, pass proportional minimum outputs to each leg, and use checked addition for total output.
- Router tests now configure mock core/AMM addresses and explicit cross-call return amounts for successful route cases; the no-simulation-fallback regression now asserts code `7` and no swap-count/volume mutation.

DEX Router validation result:

- Baseline `cd contracts/dex_router && cargo test --release` passed before the patch: 32 tests.
- Interim `cd contracts/dex_router && cargo test --release` failed as expected while old tests still expected zero-output success.
- Final `cd contracts/dex_router && cargo test --release` passed: 32 tests.
- `cd contracts/dex_router && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

DEX Margin follow-up question:

- Does `dex_margin` preserve margin and total-open-interest invariants before and after position lifecycle changes?

DEX Margin provisional answer:

- No. Current code allowed a zero-size open path, used `u64` multiplication while calculating required margin, and added open interest at entry notional but decremented it with current mark notional after price movement.

DEX Margin evidence:

- `open_position_with_mode` accepted `size == 0`, which created degenerate position state instead of rejecting the request.
- Required margin was derived from `notional * initial_margin_bps` in `u64`, so a large but otherwise under-cap notional could overflow before the division.
- `close_position`, `liquidate`, and `partial_close` removed open interest using current mark price. A price move after entry could leave stale open interest or over-subtract unrelated open interest because the increment happened at entry notional.

DEX Margin fix:

- Added checked notional calculation and rejected zero-size opens.
- Required margin now uses `u128` multiplication before division.
- Close, liquidation, and partial-close paths now decrement total open interest by entry notional, matching the amount added on open.
- Added focused regressions for zero-size rejection, large required-margin calculation, and close/partial-close open-interest accounting after price movement.

DEX Margin validation result:

- `cd contracts/dex_margin && cargo test open_position_zero --release` passed.
- `cd contracts/dex_margin && cargo test open_interest --release` passed.
- `cd contracts/dex_margin && cargo test required_margin_uses_u128 --release` passed.
- `cd contracts/dex_margin && cargo test --release` passed: 113 unit tests and 28 adversarial tests.
- Interim `cd contracts/dex_margin && cargo fmt -- --check` failed on rustfmt wrapping after the patch; fixed with `cargo fmt`.
- Final `cd contracts/dex_margin && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

DEX Rewards follow-up question:

- Can `dex_rewards` lose claimable rewards or corrupt counters when token/native transfers return a failure status or reward inputs hit release-mode arithmetic edges?

DEX Rewards provisional answer:

- Yes. Claim paths only rejected `Err(_)`, not `Ok(false)`, and several reward/counter calculations used unchecked or saturating arithmetic in places where a wrapped or silently capped value would corrupt reward accounting.

DEX Rewards evidence:

- `transfer_token_or_native` returns `CallResult<bool>`. `claim_trading_rewards`, `claim_lp_rewards`, and `claim_referral_rewards` previously zeroed pending balances after any `Ok(_)`, including `Ok(false)` failure statuses from token contracts.
- `record_trade` accepted zero fee or zero volume, so an authorized caller could mutate global trade stats without real fee-mining input; a zero-volume first trade also left per-trader volume at zero, allowing repeated unique-trader count increments for the same address.
- Referral bonus math used `fee_paid * effective_rate / 10_000` in `u64`, and LP reward accrual used `liquidity * rate / 1_000_000_000` in `u64`, which can wrap in release builds.
- `register_referral` incremented the referrer count with unchecked `count + 1`.

DEX Rewards fix:

- Claim paths now require `Ok(true)` from `transfer_token_or_native`; `Ok(false)` and `Err(_)` both return code `4` before clearing pending/earned rewards.
- `record_trade` rejects zero trader, fee, or volume before any stats mutation, computes all counter and pending changes with checked arithmetic, uses `u128` basis-point reward math, and applies mutations only after all checks pass.
- Referral bonuses and LP accrual now use checked `u128` multiply/divide helpers and return code `7` when a result cannot be represented.
- `register_referral` now checks the referrer count increment before writing the referral relationship.
- `get_pending_rewards` now saturates the query-only sum instead of wrapping.

DEX Rewards validation result:

- Baseline `cd contracts/dex_rewards && cargo test --release` passed before the patch: 51 tests.
- `cd contracts/dex_rewards && cargo test false_transfer_status --release` passed: 3 tests.
- `cd contracts/dex_rewards && cargo test u128 --release` passed: 2 tests.
- `cd contracts/dex_rewards && cargo test rejects --release` passed: 7 tests.
- Final `cd contracts/dex_rewards && cargo test --release` passed: 59 tests.
- `cd contracts/dex_rewards && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

DEX Governance follow-up question:

- Does `dex_governance` execute exactly what voters approved, with fail-closed dependency wiring and downstream execution status checks?

DEX Governance provisional answer:

- No. Fee proposals were not reputation-gated, new-pair execution discarded the proposed quote token, the core execution target had no callable bootstrap setter, and downstream nonzero return statuses could still be treated as executed work.

DEX Governance evidence:

- `propose_new_pair` validated the caller-supplied quote token but proposal storage only retained the base token; `execute_proposal` then used the current preferred quote instead of the voted quote.
- `CORE_ADDRESS_KEY` was read during execution, but the contract had no public/admin setter and genesis only configured LichenID plus allowed quotes.
- `propose_fee_change` did not call `verify_reputation`, despite new-pair proposals and votes requiring LichenID reputation.
- Fee proposals could carry maker rebates below the intended 1% bound; DEX core also only rejected positive maker fees above `MAX_FEE_BPS`, not overly negative rebates.
- `execute_proposal` only treated cross-contract `Err(_)` as failure. A downstream contract returning a nonzero error code in return data could still be marked executed.

DEX Governance fix:

- Added one-time admin `set_core_address` / opcode `20` and wired it in genesis for `dex_governance(core)`.
- New-pair proposals now store the submitted quote token under a proposal-specific key; execution uses that stored quote, with preferred-quote fallback only for legacy proposals.
- Fee-change proposals now require LichenID reputation, reject pair id zero, and enforce taker/maker fee bounds.
- Proposal execution now treats empty return data as legacy success, but decodes 4- or 8-byte status return data and keeps the proposal `PASSED`/retryable on nonzero downstream status.
- DEX core direct `update_pair_fees` now rejects maker rebates below `-MAX_FEE_BPS`, and maker rebate math uses `unsigned_abs`.

DEX Governance validation result:

- Baseline `cd contracts/dex_governance && cargo test --release` passed before the patch: 39 tests.
- `cd contracts/dex_governance && cargo test core_address --release` passed: 2 tests.
- `cd contracts/dex_governance && cargo test propose_fee_change --release` passed: 3 tests.
- `cd contracts/dex_governance && cargo test execute_new_pair --release` passed: 3 tests.
- `cd contracts/dex_core && cargo test update_fees --release` passed: 1 unit test and 3 adversarial tests.
- Final `cd contracts/dex_governance && cargo test --release` passed: 44 tests.
- Final `cd contracts/dex_core && cargo test --release` passed: 85 unit tests and 33 adversarial tests.
- `cargo test -p lichen-genesis --release` passed: 7 lib tests and 1 bin test.
- `cd contracts/dex_governance && cargo fmt -- --check` passed.
- `cd contracts/dex_core && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

DEX Analytics follow-up question:

- Can `dex_analytics` be spoofed or have stats corrupted by direct callers or release-mode arithmetic wraps?

DEX Analytics provisional answer:

- Yes. Any trader could call `record_trade` or `record_pnl` for themselves, which let arbitrary wallets inject fake volume, prices, candles, leaderboard entries, and PnL. Several counters also used unchecked addition.

DEX Analytics evidence:

- `record_trade` accepted calls when `real_caller == trader`, even if `AUTHORIZED_CALLER_KEY` was unset or configured for `dex_core`.
- `record_pnl` used the same direct-trader allowance, allowing self-reported realized PnL.
- `record_trade`, 24h stats, candle volume/count, trader volume/trade count, and unique-trader counters used unchecked `+` in release builds.
- Biased PnL updates cast signed overflow/underflow back to `u64`.

DEX Analytics fix:

- `record_trade` and `record_pnl` now require a nonzero configured authorized caller and reject any direct trader write.
- `record_trade` rejects pair id zero and zero trader addresses in addition to zero price/volume.
- Analytics counters and volume accumulators now saturate instead of wrapping.
- PnL updates now check the biased signed result and return code `4` before storing out-of-range values.
- Unit tests now use the authorized-caller path by default and include explicit unauthorized/direct-ingestion regressions.

DEX Analytics validation result:

- Baseline `cd contracts/dex_analytics && cargo test --release` passed before the patch: 26 tests.
- `cd contracts/dex_analytics && cargo test unauthorized --release` passed but matched 0 tests; reran with exact filters below.
- `cd contracts/dex_analytics && cargo test rejects_direct --release` passed: 1 test.
- `cd contracts/dex_analytics && cargo test saturates --release` passed: 1 test.
- `cd contracts/dex_analytics && cargo test record_pnl --release` passed: 1 test.
- Final `cd contracts/dex_analytics && cargo test --release` passed: 29 tests.
- `cd contracts/dex_analytics && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `dex_core` fixed and validated.
- `dex_amm` fixed and validated.
- `dex_router` fixed and validated.
- `dex_margin` fixed and validated.
- `dex_rewards` fixed and validated.
- `dex_governance` fixed and validated.
- `dex_analytics` fixed and validated.
- Core DeFi contract family completed.
- Continue Part 5 with markets: `lichenswap`, `thalllend`, `prediction_market`, and `sporepump`.

### Part 5 Resume Point After Task 5.3

Current next investigation target:

- Continue Part 5 with the markets contract family: `lichenswap`, then `thalllend`, `prediction_market`, and `sporepump`.

### Task 5.4 - Markets Contract Family

LichenSwap question:

- Can `lichenswap` lose funds or accept invalid state when cross-token transfers return `Ok(false)`, reputation discounts are overbounded, or flash-loan arithmetic reaches release-mode overflow edges?

LichenSwap provisional answer:

- Yes. Several outbound transfer helpers treated `Ok(false)` as success, discount inputs had no upper bound, and flash-loan reserve/fee math used narrow unchecked arithmetic in paths that affect borrow limits and repayment accounting.

LichenSwap evidence:

- `transfer_out` returned success for any `Ok(_)` response from `transfer_token_or_native`, which meant `Ok(false)` could let later accounting proceed as if funds moved.
- `set_reputation_discount` accepted arbitrary basis-point values and `get_reputation_bonus` added bonuses with unchecked `u64` math.
- Flash-loan max borrow calculation multiplied `reserve * MAX_FLASH_LOAN_PERCENT` in `u64`.
- Flash-loan borrow and repay paths did not explicitly fail closed when `reserve + fee`, `amount + fee`, or reserve fee collection overflowed.
- TWAP/swap counters and protocol fee accounting used unchecked additions/subtractions in release builds.

LichenSwap fix:

- `transfer_out` now requires explicit `Ok(true)` transfer status; `Ok(false)` and `Err(_)` both return failure code `31`.
- TWAP snapshot count, swap count, protocol-fee accrual, and fee deductions use saturating or checked arithmetic.
- Reputation discounts are capped at 10,000 bps, and bonus math uses `u128`/checked addition.
- Flash-loan max borrow uses `u128`, borrow rejects impossible `reserve + fee`, and repay checks required repayment plus fee collection before mutating reserve accounting.

LichenSwap validation result:

- Baseline `cd contracts/lichenswap && cargo test --release` passed before the patch: 32 tests.
- `cd contracts/lichenswap && cargo test flash_loan --release` passed: 7 tests.
- `cd contracts/lichenswap && cargo test reputation_discount --release` passed: 2 tests.
- Final `cd contracts/lichenswap && cargo test --release` passed: 34 tests.
- `cd contracts/lichenswap && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

LichenSwap residual note:

- This patch intentionally stayed scoped to transfer-status decoding, bounded discounts, and flash/protocol arithmetic guards. It did not perform a broad legacy AMM accounting rewrite because the SDK `Pool` helper mutates storage before some outbound transfers.

Status:

- `lichenswap` fixed and validated.
- Continue Part 5 markets with `thalllend`, then `prediction_market`, and `sporepump`.

### Part 5 Resume Point During Task 5.4

Current next investigation target:

- Continue Part 5 with the markets contract family: `thalllend`, then `prediction_market`, and `sporepump`.

ThallLend question:

- Can `thalllend` accept unsafe lending state through unchecked accounting, stale/invalid collateral valuation, or failed transfer paths?

ThallLend provisional answer:

- Yes. Configured oracle failures and zero prices could fall back to 1:1 collateral valuation, outgoing token transfers accepted `Ok(false)`, and several lending, utilization, borrow-index, and flash-loan arithmetic paths could wrap or truncate in release builds.

ThallLend evidence:

- `get_oracle_price` used a 1:1 fallback even after an oracle feed was configured but returned no data or zero data.
- `transfer_out` treated any `Ok(_)` from `transfer_token_or_native` as success, so `Ok(false)` could let withdraw/borrow/liquidation/reserve paths proceed as if funds moved.
- Deposit totals and per-account deposits used saturating adds; this could silently clamp balances instead of rejecting impossible accounting.
- Utilization, collateral-factor, liquidation-threshold, health-factor, interest, and borrow-index settlement math used narrow or truncating arithmetic in release builds.
- `flash_repay` computed `borrowed + fee` unchecked, so an overflow could clear the active flash-loan state with insufficient repayment.
- Flash-loan borrow/repay paths were mutating but did not use the contract's reentrancy guard.

ThallLend fix:

- Configured oracle reads now fail closed when the oracle call fails, returns too little data, or returns zero; the 1:1 fallback is preserved only for the unconfigured-oracle bootstrap case.
- `transfer_out` now requires explicit `Ok(true)` status and returns code `31` for `Ok(false)` or call errors.
- Deposit accounting now rejects total or per-account overflow with code `5`; counters saturate instead of wrapping.
- Utilization, collateral limits, liquidation limits, health factors, interest accrual, interest quotes, and borrow-index settlement now use `u128` intermediates and saturating conversion to `u64`.
- Flash repayment now checks `borrowed + fee` before comparison and leaves active loan state intact on overflow; flash borrow/repay now enter and exit the reentrancy guard around mutating state.

ThallLend validation result:

- Baseline `cd contracts/thalllend && cargo test --release` passed before the patch: 52 tests.
- `cd contracts/thalllend && cargo test overflow --release` passed: 3 tests.
- `cd contracts/thalllend && cargo test oracle --release` passed: 4 tests.
- `cd contracts/thalllend && cargo test transfer_status --release` passed: 1 test.
- `cd contracts/thalllend && cargo test utilization --release` passed: 1 test.
- `cd contracts/thalllend && cargo test liquidation_limit --release` passed: 1 test.
- Final `cd contracts/thalllend && cargo test --release` passed: 61 tests.
- `cd contracts/thalllend && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

Status:

- `lichenswap` fixed and validated.
- `thalllend` fixed and validated.
- Continue Part 5 markets with `prediction_market`, then `sporepump`.

### Part 5 Resume Point During Task 5.4 - After ThallLend

Current next investigation target:

- Continue Part 5 with the markets contract family: `prediction_market`, then `sporepump`.

Prediction Market question:

- Can `prediction_market` mis-settle, mis-account, or accept unsafe market state through unchecked arithmetic, transfer-status handling, or oracle/settlement edge cases?

Prediction Market provisional answer:

- Yes. lUSD payout helpers accepted `Ok(false)` as success, complete-set redemption mutated positions/pools/collateral before confirming the payout, and hot analytics/collateral counters used release-mode wrapping additions.

Prediction Market evidence:

- `transfer_lusd_out` returned success for any `Ok(_)` from `call_token_transfer`, so token contracts returning a false/error status could still clear redemption, sell, LP-withdraw, or resolver payout state.
- `redeem_complete_set` burned all outcome positions, reduced pool accounting, and reduced market/global collateral before attempting the lUSD transfer; a transfer failure left the user with no retryable complete set.
- Buy/add/mint paths used unchecked collateral additions in cap checks, allowing overflow to bypass the cap comparison before escrow.
- Trader stats, market trader counts, 24h volume, price snapshot count, user/category/active indexes, dispute count, fee counters, and several position/pool totals used unchecked addition in release builds.

Prediction Market fix:

- `transfer_lusd_out` now requires explicit `Ok(true)` transfer status and fails closed on `Ok(false)` or call errors.
- `redeem_complete_set` now preflights positions/pools, transfers lUSD first, and only then mutates positions, pool totals, market collateral, and global collateral.
- Initial liquidity, add liquidity, buy, and mint complete-set paths now check collateral additions before escrow where those additions gate circuit breakers.
- Hot analytics/index counters and accounting additions now use saturating arithmetic; price-move pause slot addition saturates; submit-resolution dispute deadline uses checked addition.

Prediction Market validation result:

- Baseline `cd contracts/prediction_market && cargo test --release` passed before the patch: 72 unit tests, 49 adversarial tests, 75 core tests, and 36 resolution tests.
- `cd contracts/prediction_market && cargo test transfer_status --release` passed: 1 test.
- `cd contracts/prediction_market && cargo test failed_transfer_preserves_state --release` passed: 1 test.
- `cd contracts/prediction_market && cargo test collateral_overflow --release` passed: 1 test.
- `cd contracts/prediction_market && cargo test analytics_counters --release` passed: 1 test.
- Final `cd contracts/prediction_market && cargo test --release` passed: 76 unit tests, 49 adversarial tests, 75 core tests, and 36 resolution tests.
- `cd contracts/prediction_market && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.

Status:

- `lichenswap` fixed and validated.
- `thalllend` fixed and validated.
- `prediction_market` fixed and validated.
- Continue Part 5 markets with `sporepump`.

### Part 5 Resume Point During Task 5.4 - After Prediction Market

Current next investigation target:

- Continue Part 5 with the markets contract family: `sporepump`.

SporePump question:

- Can `sporepump` misprice, lose fees, or corrupt launch/AMM state through unchecked math, failed transfer status, or lifecycle edge cases?

SporePump provisional answer:

- Yes. Outgoing LICN transfers accepted false status responses, creation fee accounting trusted the caller-supplied `fee_paid` parameter, cooldown deadlines could wrap, token-count creation could wrap, and bonding-curve math could truncate at extreme values.

SporePump evidence:

- `transfer_licn_out` returned success for any `Ok(_)` from `transfer_token_or_native`, so `Ok(false)` could let sells or fee withdrawals appear successful.
- `create_token` checked `get_value()` but credited `cp_fees_collected` with caller-supplied `fee_paid`, allowing fee accounting to exceed actual payment.
- `TOKEN_COUNT_KEY + 1` was unchecked, so a saturated counter could wrap to token id zero.
- Buy/sell cooldown comparisons used `last_buy_ts + cooldown`, which could wrap and bypass an intentionally long cooldown.
- Bonding-curve cost/refund and market-cap helpers cast large `u128` values down to `u64`, which could truncate release-mode quotes/state.

SporePump fix:

- `transfer_licn_out` now requires explicit `Ok(true)` status and fails closed on `Ok(false)` or call errors.
- Token creation now rejects token-counter overflow and credits exactly `CREATION_FEE` to platform fees.
- Buy/sell cooldown deadlines use saturating addition; fee calculations use `u128` intermediates.
- Raised-amount and buyer-balance overflow fail closed; sell/fee-withdraw transfer failures continue to revert bookkeeping.
- Bonding-curve cost/refund, current price, market cap, and buy-quote math use saturating conversion instead of truncating casts.

SporePump validation result:

- Baseline `cd contracts/sporepump && cargo test --release` passed before the patch: 47 tests.
- `cd contracts/sporepump && cargo test false --release` passed: 2 tests.
- `cd contracts/sporepump && cargo test create_token --release` passed: 5 tests.
- `cd contracts/sporepump && cargo test cooldown_overflow --release` passed: 1 test.
- `cd contracts/sporepump && cargo test bonding_curve_math --release` initially failed because the test expected `current_price(u64::MAX)` to saturate even though the configured slope keeps it finite; the expectation was corrected.
- `cd contracts/sporepump && cargo test bonding_curve_math --release` passed after correction: 1 test.
- Final `cd contracts/sporepump && cargo test --release` passed: 53 tests.
- `cd contracts/sporepump && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenswap` fixed and validated.
- `thalllend` fixed and validated.
- `prediction_market` fixed and validated.
- `sporepump` fixed and validated.
- Markets contract family completed.
- Continue Part 5 with identity/governance: `lichenid`, then `lichendao`, and `lichenoracle`.

### Part 5 Resume Point After Task 5.4

Current next investigation target:

- Continue Part 5 with the identity/governance contract family: `lichenid`, then `lichendao`, and `lichenoracle`.

### Part 5 Resume Point During Task 5.5 - After LichenID

Current next investigation target:

- Continue Part 5 with the identity/governance contract family: `lichendao`, then `lichenoracle`.

LichenID question:

- Can `lichenid` corrupt identity/name/reputation state, lose auction refunds, or expose privileged identity naming through unchecked arithmetic, weak caller binding, cross-contract false-success status, or bad vouch indexing?

LichenID provisional answer:

- Yes. The baseline suite was green, but the source had several mainnet-readiness gaps: reserved-name admin calls trusted the args buffer without binding it to `get_caller()`, premium-name outbid refunds accepted any `Ok(_)`, failed/misconfigured refund paths mutated auction state before failing, vouch-given indexing reused the received-vouch count, and multiple counters/deadlines could wrap.

LichenID evidence:

- `admin_register_reserved_name()` only checked that the first 32 arg bytes matched the stored admin, so a non-admin caller could spoof the admin in args.
- `bid_name_auction()` updated the highest bidder before refunding the previous bidder and treated `Ok(false)` from `transfer_token_or_native` as success.
- `vouch()` used the identity record's received-vouch count as the `vouch_given` index and then incremented that same received-vouch field on the voucher's record.
- Registration/vouch cooldown checks used `last_ts + cooldown`; identity/name/attestation/achievement/contribution counters and name expiry calculations had unchecked additions.
- RPC LichenID skill attestation lookup used an older 8-byte truncated skill hash, while the contract now stores count keys under the 16-byte FNV-1a hash with a 16-byte legacy fallback.

LichenID fix:

- Reserved-name admin registration now verifies `get_caller()` matches the stored admin supplied in args and rejects invalid agent types.
- Outbid refunds now require explicit `Ok(true)` before the auction record changes; missing token/self-address config and `Ok(false)` transfer status leave the previous high bid intact.
- Given-vouch indexing now has a separate `vouch_given_count:{voucher}` key; received-vouch counts are only incremented for the vouchee.
- Registration/vouch cooldown deadlines use saturating addition; identity/name counters and name expiry math use checked arithmetic where state must remain retryable; achievement/contribution counters saturate where the write is auxiliary.
- RPC now reads 16-byte FNV attestation count keys with legacy fallback and uses `vouch_given_count` for direct given-vouch scans; core processor achievement/contribution counter increments now saturate.

LichenID validation result:

- Baseline `cd contracts/lichenid && cargo test --release` passed before the patch: 65 tests.
- Focused release tests passed: `vouch_uses_separate_given_count`, `register_identity_counter_overflow`, `register_cooldown_overflow`, `false_refund_status`, `admin_register_reserved_name`, `register_name_expiry_overflow`, `finalize_name_auction_rejects_caller_mismatch`, `refund_requires_token_config`, and `refund_requires_self_address`.
- Final `cd contracts/lichenid && cargo test --release` passed: 71 tests.
- Focused RPC release tests passed: `test_get_lichenid_skills_with_attestations`, `test_get_lichenid_vouches_bidirectional`, and `test_get_lichenid_profile_and_directory`.
- `cargo check -p lichen-rpc --tests` passed.
- `cargo test -p lobstercove-lichen-core achievement --release` passed as a release compile/filter gate; no tests matched the filter.
- `cargo fmt --all -- --check` initially failed on a single RPC helper wrap; corrected and rerun passed.
- `cd contracts/lichenid && cargo fmt -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenid` fixed and validated.
- Continue Part 5 identity/governance with `lichendao`, then `lichenoracle`.

### Part 5 Resume Point During Task 5.5 - After LichenDAO

Current next investigation target:

- Continue Part 5 with the remaining identity/governance contract: `lichenoracle`.

LichenDAO question:

- Can `lichendao` drain treasury funds, lose proposal stake refunds, or corrupt governance state through unbound proposal actions, unchecked arithmetic, false-success transfers, or unsafe pointer entrypoints?

LichenDAO provisional answer:

- Yes. The baseline suite was green, but `treasury_transfer` only checked that a proposal had been executed; it did not bind the requested token, recipient, or amount to the proposal's approved action hash. The slice also had unchecked proposal counter/end-time/vote/veto arithmetic, stored the fixed `PROPOSAL_STAKE` even when a different configured threshold was escrowed, and marked cancellations before refund success.

LichenDAO evidence:

- Any caller could invoke `treasury_transfer(proposal_id, token, recipient, amount)` after an executed proposal and choose arbitrary transfer parameters; the proposal's target/action hash was not consulted.
- `create_proposal_typed()` escrowed `min_proposal_threshold` but stored/refunded `PROPOSAL_STAKE`, so custom thresholds could under-refund or over-refund.
- `proposal_count += 1`, `now + voting_period`, `end_time + execution_delay`, vote totals, and veto totals used unchecked addition.
- `cancel_proposal()` marked a proposal cancelled before attempting the stake refund, so failed transfer status could strand the stake and leave no retry path.
- Several reviewed entrypoints copied fixed-size pointers without null checks.

LichenDAO fix:

- Treasury transfers now require the executed proposal's target to be the DAO contract itself and require `sha256("treasury_transfer\0" + token + recipient + amount)` to match the proposal's stored action hash.
- Proposal counter, proposal end time, timelock deadline, vote totals, total votes, and veto totals now fail closed on overflow; rejected proposal-counter overflow happens before escrow.
- Proposals store the actual configured stake threshold and refund that amount.
- Cancellation refunds before marking the proposal cancelled; refund failure leaves the proposal cancellable.
- Post-execution stake refund failure records `stake_refund_due:<proposal_id>` and exposes `claim_proposal_stake_refund` for proposer retry.
- Reviewed pointer entrypoints now reject null pointers, and legacy 210-byte proposal reads are padded before returning 212 bytes.

LichenDAO validation result:

- Baseline `cd contracts/lichendao && cargo test --release` passed before the patch: 13 tests.
- Final `cd contracts/lichendao && cargo test --release` passed: 19 tests.
- Added focused regressions for actual stake threshold storage, proposal-counter overflow before escrow, vote-total overflow without recording a vote, cancellation refund failure preserving state, treasury transfer rejection on action mismatch, and treasury transfer success on matching approved action.
- `cd contracts/lichendao && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenid` fixed and validated.
- `lichendao` fixed and validated.
- Continue Part 5 identity/governance with `lichenoracle`.

### Part 5 Resume Point After Task 5.5 - Identity/Governance Complete

Current next investigation target:

- Continue Part 5 with the NFT/marketplace contract family: `lichenmarket`, then `lichenauction`, and `lichenpunks`.

LichenOracle question:

- Can `lichenoracle` publish unsafe oracle data or corrupt feed, randomness, reporter, or attestation state through unbounded inputs, unchecked counters, zero/stale price values, or lossy authorization keys?

LichenOracle provisional answer:

- Yes. The baseline suite was green, but the source accepted unbounded asset/query/data lengths, accepted zero prices, allowed excessive decimals to drive overflowing `10^decimals` display math, let a requester overwrite a pending randomness commit, did not verify attestation data against the submitted hash, could wrap attestation signature counts after writing dedup state, and keyed reporters by only the first two address bytes.

LichenOracle evidence:

- Several exported functions allocated vectors directly from caller-supplied lengths and copied fixed pointers without null checks.
- `submit_price()` accepted `price == 0` and any `decimals`, then logged `10u64.pow(decimals as u32)`.
- `commit_randomness()` overwrote the requester's pending commit key without requiring reveal or completion first.
- `submit_attestation()` trusted the caller-supplied `data_hash`, wrote the attester dedup key before incrementing the count, and used `attestation[32] + 1`.
- `add_reporter()` / `remove_reporter()` stored `reporter_{first_byte}{second_byte}`, colliding reporters with the same first two bytes.

LichenOracle fix:

- Added bounded/null-safe readers for assets, query types, attestation data, and fixed 32-byte addresses.
- `submit_price()` now rejects zero prices and decimals above 18; query/feed/attestation stats use saturating increments.
- Pending randomness commits cannot be overwritten before reveal.
- Attestation data must SHA-256 hash to the submitted hash, signature count overflow fails before dedup state is written, and `min_signatures == 0` does not verify.
- Reporter keys now use the full 32-byte address hex.

LichenOracle validation result:

- Baseline `cd contracts/lichenoracle && cargo test --release` passed before the patch: 27 tests.
- Final `cd contracts/lichenoracle && cargo test --release` passed: 33 tests.
- Added focused regressions for oversized asset rejection, zero/excessive-decimal price rejection, duplicate pending randomness commit rejection, attestation hash verification, attestation count overflow without dedup writes, and full-address reporter-key separation.
- `cd contracts/lichenoracle && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenid` fixed and validated.
- `lichendao` fixed and validated.
- `lichenoracle` fixed and validated.
- Identity/governance contract family completed.
- Continue Part 5 with NFT/marketplace: `lichenmarket`, then `lichenauction`, and `lichenpunks`.

### Part 5 Resume Point During Task 5.6 - After LichenMarket

Current next investigation target:

- Continue Part 5 NFT/marketplace with `lichenauction`, then `lichenpunks`.

LichenMarket question:

- Can `lichenmarket` lose buyer, seller, bidder, or royalty funds or corrupt marketplace/auction state when cross-contract transfers fail, offers expire, or counters/time arithmetic wraps?

LichenMarket provisional answer:

- Yes. The baseline suite was green, but direct offer acceptance paid the seller before moving the NFT, missed a reentrancy-exit path on missing offers, ignored offer expiry records, auction bidding ignored previous-bid refund failure, listings could be zero-price, and listing/sale/auction time arithmetic used unchecked increments.

LichenMarket evidence:

- `accept_offer()` transferred payment from buyer to seller before `call_nft_transfer`; if the NFT transfer failed, seller payment was already attempted and the offer stayed active.
- `accept_offer()` returned from the missing-offer branch without clearing the reentrancy key.
- `make_offer_with_expiry()` stored an expiry, but `accept_offer()` ignored it.
- `place_bid()` escrowed the new bid and then ignored failed refund of the previous highest bidder before updating the auction.
- `list_nft()` / `list_nft_with_royalty()` allowed price zero, while update paths rejected zero.
- Listing and sale counters used `+ 1`; auction end time and anti-sniping extension used unchecked addition.

LichenMarket fix:

- `accept_offer()` now verifies seller ownership, escrows the full payment to marketplace escrow before NFT transfer, refunds the buyer if NFT transfer fails, rejects expired offers, and releases the reentrancy guard on missing offers.
- Failed seller/royalty payout releases now record `unpaid_payout:<token>:<recipient>` amounts instead of silently clearing state.
- `place_bid()` now fails closed if the previous highest bidder cannot be refunded and attempts to refund the new bidder before preserving previous auction state.
- Zero-price listings are rejected before NFT owner lookup.
- Listing/sale counters use saturating increments; auction end/anti-sniping timestamps use checked addition.

LichenMarket validation result:

- Baseline `cd contracts/lichenmarket && cargo test --release` passed before the patch: 28 tests.
- Final `cd contracts/lichenmarket && cargo test --release` passed: 34 tests.
- Added focused regressions for zero-price listing rejection before owner lookup, offer escrow/refund behavior when NFT transfer fails, successful offer escrow/inactivation, expired offer rejection, auction end-time overflow rejection, and previous-bid refund failure preserving highest-bid state.
- `cd contracts/lichenmarket && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenmarket` fixed and validated.
- Continue Part 5 NFT/marketplace with `lichenauction`, then `lichenpunks`.

### Part 5 Resume Point During Task 5.6 - After LichenAuction

Checkpoint time: 2026-04-26 00:04:12 +0400

Current next investigation target:

- Continue Part 5 NFT/marketplace with `lichenpunks`.

LichenAuction question:

- Can `lichenauction` lose bidder, offerer, seller, or royalty funds or corrupt auction state when escrow, refund, NFT transfer, payout, time, or counter arithmetic fails?

LichenAuction provisional answer:

- Yes. The baseline suite was green, but bid replacement refunded the previous highest bidder before escrowing the replacement bid, finalization released seller/royalty funds before NFT transfer, offer acceptance paid directly from the offerer before NFT transfer, and several time/counter additions were unchecked.

LichenAuction evidence:

- `create_auction()` accepted zero minimum bids and used unchecked `now + duration`.
- `place_bid()` used unchecked 5% increment math, refunded the previous highest bidder before replacement escrow, and extended anti-snipe timestamps with unchecked addition.
- `finalize_auction()` marked auctions inactive and paid seller/royalty proceeds before `call_nft_transfer`; NFT transfer failure could leave paid proceeds with the NFT still owned by the seller.
- `accept_offer()` transferred seller/fee/royalty amounts from the offerer before moving the NFT; NFT transfer failure could charge the offerer while the offer remained active.
- Collection/global counters used unchecked increments, and several exported functions copied address pointers without null checks.

LichenAuction fix:

- Added null-safe address reads, checked auction/offer expiry math, checked bid-increment and anti-snipe extension math, zero bid/offer rejection, saturating counters, and unpaid-payout accounting.
- `place_bid()` now escrows the replacement bid first; if previous-bidder refund fails it refunds or records the replacement bidder payout and leaves the old highest bid unchanged.
- `finalize_auction()` now transfers the NFT before releasing proceeds; NFT transfer failure leaves the auction active, while post-NFT seller/royalty release failures record unpaid payouts and finalize the auction.
- `accept_offer()` now reentrancy-guards the flow, escrows the full offer payment, refunds or records the offerer if NFT transfer fails, and only consumes the offer after NFT transfer succeeds.
- Reserve-not-met finalization now refunds before marking inactive, and operations requiring escrow reject missing/invalid marketplace escrow configuration instead of using the zero address.

LichenAuction validation result:

- Baseline `cd contracts/lichenauction && cargo test --release` passed before the patch: 34 tests.
- Final `cd contracts/lichenauction && cargo test --release` passed: 41 tests.
- Added focused regressions for auction end-time overflow, zero/overflow offer rejection, failed previous-bidder refund preserving highest-bid state, offer refund on NFT-transfer failure, successful offer escrow/inactivation, finalize NFT-transfer failure preserving active auction, and unpaid seller payout recording after NFT transfer.
- `cd contracts/lichenauction && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenmarket` fixed and validated.
- `lichenauction` fixed and validated.
- Continue Part 5 NFT/marketplace with `lichenpunks`.

### Part 5 Resume Point After Task 5.6 - NFT/Marketplace Complete

Checkpoint time: 2026-04-26 00:08:50 +0400

Current next investigation target:

- Continue Part 5 with agent/storage/payments: `bountyboard`, `compute_market`, `moss_storage`, `sporepay`, and `sporevault`.

LichenPunks question:

- Can `lichenpunks` mint, transfer, burn, approve, or update collection settings through spoofed caller input, uninitialized minter state, unbounded metadata/base URI reads, stale metadata keys, pause bypasses, or wrapped counters?

LichenPunks provisional answer:

- Yes. The baseline suite was green, but `transfer_from()` trusted a caller pointer without checking `get_caller()`, mint could self-mint before collection initialization, metadata/base URI reads were unbounded, the `get_punk_metadata()` alias used the wrong storage key, mutable flows missed pause/null-pointer coverage, and stats counters used unchecked increments.

LichenPunks evidence:

- `transfer_from()` accepted a caller address from calldata and passed it into `NFT::transfer_from()` without verifying the transaction signer.
- `mint()` allowed `caller == to` self-minting even if no minter had been initialized.
- `mint()` and `set_base_uri()` allocated directly from caller-provided lengths and copied raw pointers without null/size checks.
- `get_punk_metadata()` read `nft_meta_{token_id}`, while the SDK stores NFT metadata under `metadata:` plus the little-endian token id.
- `approve()`, `transfer_from()`, and `burn()` were not pause-gated, and transfer/burn stats used `+ 1`.

LichenPunks fix:

- Added null-safe address/byte readers, initialization gating before mint, zero recipient rejection, bounded metadata/base URI reads, safe `stored_u64`, and saturating transfer/burn counters.
- `transfer_from()` now requires `get_caller()` to match the caller pointer and is pause-gated; successful `transfer_from()` increments transfer stats.
- `approve()` and `burn()` are pause-gated, `burn()` uses saturating stats, and transfer paths precheck recipient balance overflow.
- `set_royalty()` now caps royalties at 1000 bps, `set_max_supply()` rejects positive caps below current supply, and `set_base_uri()` rejects oversized/invalid URI input.
- `get_punk_metadata()` now uses the actual SDK metadata key.

LichenPunks validation result:

- Baseline `cd contracts/lichenpunks && cargo test --release` passed before the patch: 21 tests.
- Final `cd contracts/lichenpunks && cargo test --release` passed: 28 tests.
- Added focused regressions for mint-before-initialize rejection, oversized metadata rejection, correct metadata alias lookup, spoofed `transfer_from()` rejection, paused `transfer_from()`/`burn()` rejection, transfer counter saturation, and admin URI/supply/royalty bounds.
- `cd contracts/lichenpunks && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `lichenmarket` fixed and validated.
- `lichenauction` fixed and validated.
- `lichenpunks` fixed and validated.
- NFT/marketplace contract family completed.
- Continue Part 5 with agent/storage/payments: `bountyboard`, `compute_market`, `moss_storage`, `sporepay`, and `sporevault`.

### Part 5 Resume Point During Task 5.7 - After BountyBoard

Checkpoint time: 2026-04-26 00:12:21 +0400

Current next investigation target:

- Continue Part 5 agent/storage/payments with `compute_market`, then `moss_storage`, `sporepay`, and `sporevault`.

BountyBoard question:

- Can `bountyboard` strand reward escrow, complete/cancel without payout/refund, wrap bounty/stat counters, or accept unsafe admin and pointer inputs?

BountyBoard provisional answer:

- Yes. The baseline suite was green, but bounty creation required attached value while approval/cancel with no configured reward token performed no native transfer, malformed token config could silently skip payout/refund, `bounty_count` and stats used unchecked increments, and several admin/caller pointer reads were unchecked.

BountyBoard evidence:

- `create_bounty()` checked `get_value() >= reward_amount`, but `approve_work()` and `cancel_bounty()` only transferred when `bounty_token_addr` existed; missing token config completed/cancelled without moving the native escrow.
- `approve_work()` did not validate loaded submission length before reading worker bytes.
- `create_bounty()` used `bounty_id + 1` unchecked, and completed/cancel counters used `+ 1`.
- `set_platform_fee()` had no cap and `bb_pause()` / `bb_unpause()` / fee paths used `unwrap_or_default()` for admin state.
- Many exported functions copied 32-byte pointers without null checks.

BountyBoard fix:

- Added null-safe address reads, safe `stored_u64`, saturating stat counters, checked bounty-count increment, and strict admin-state checks.
- Missing reward-token config now means native payout/refund via the zero-address system token; malformed token config fails closed and leaves bounties open.
- `approve_work()` validates submission length and reverts completion on payout failure; `cancel_bounty()` reverts cancellation on refund failure.
- `set_platform_fee()` now caps fees at 1000 bps and requires a configured admin.

BountyBoard validation result:

- Baseline `cd contracts/bountyboard && cargo test --release` passed before the patch: 24 tests.
- Final `cd contracts/bountyboard && cargo test --release` passed: 30 tests.
- Added focused regressions for bounty-count overflow rejection, native payout/refund without token config, malformed token config fail-closed behavior, platform-fee cap/admin requirement, and stats counter saturation.
- `cd contracts/bountyboard && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `bountyboard` fixed and validated.
- Continue Part 5 agent/storage/payments with `compute_market`, then `moss_storage`, `sporepay`, and `sporevault`.

### Part 5 Resume Point During Task 5.7 - After ComputeMarket

Checkpoint time: 2026-04-26 03:49:12 +0400

Current next investigation target:

- Continue Part 5 agent/storage/payments with `moss_storage`, then `sporepay` and `sporevault`.

ComputeMarket question:

- Can `compute_market` lose provider/requester funds, accept spoofed or inactive providers, or corrupt job/provider/reputation state when escrow, payout/refund, expiry, staking, pointer input, or counter arithmetic fails?

ComputeMarket provisional answer:

- Yes. The baseline suite was green, but escrow collection accepted `Ok(false)`, `job_count` wrapped after charging escrow, inactive providers could claim jobs, refund/release paths cleared accounting before fully checking transfer outcomes, split-dispute payouts could double-pay the first successful recipient on retry, and provider/stat/platform-fee counters were not bounded.

ComputeMarket evidence:

- `submit_job()` checked only `.is_err()` from `receive_token_or_native()`, so an `Ok(false)` token response still created a job and escrow record.
- `submit_job()` incremented `job_count` with `job_id + 1` after escrow collection, so overflow could charge a requester and then wrap job IDs.
- `claim_job()` copied the provider pointer without a null check and only required a provider record to exist; inactive providers were still allowed to claim new jobs.
- `complete_job()` copied provider/result pointers without null checks and incremented provider completions with `+ 1`.
- `dispute_job()` copied requester pointers without null checks and incremented dispute stats with `+ 1`.
- `cancel_job()` and `release_payment()` cleared status/escrow before transfer and did not treat `Ok(false)` as failed payout/refund status.
- `resolve_dispute()` transferred requester funds before provider funds; if the provider leg failed, the job stayed disputed with the full escrow record and a retry could pay the requester again.
- `set_platform_fee()` had no upper bound.

ComputeMarket fix:

- Added shared `stored_u64`, saturating counter, and unpaid-payout helpers.
- `submit_job()` now checks `job_count` overflow before collecting escrow and requires explicit `Ok(true)` escrow status.
- `claim_job()` now rejects null provider pointers, corrupt provider records, and inactive providers.
- `complete_job()` and `dispute_job()` now reject null pointers and use saturating provider/dispute counters.
- `cancel_job()` restores the prior status and escrow amount if refund transfer fails, returns `Ok(false)`, or token configuration is invalid.
- `release_payment()` restores completed status and escrow amount if provider payout fails, returns `Ok(false)`, or token configuration is invalid; completion stats now saturate.
- `resolve_dispute()` now requires a valid token config when escrow exists and records an unpaid provider payout if the requester leg already succeeded but the provider leg fails, avoiding double requester payouts on retry.
- `set_platform_fee()` now rejects fees above 1000 bps.

ComputeMarket validation result:

- Baseline `cd contracts/compute_market && cargo test --release` passed before the patch: 40 tests.
- Final `cd contracts/compute_market && cargo test --release` passed: 49 tests.
- Added focused regressions for job-count overflow before escrow, `Ok(false)` escrow rejection, inactive provider claim rejection, failed cancel refund preserving job/escrow, failed release preserving completed job/escrow, partial dispute payout recording unpaid provider amount, saturating provider/dispute/completed counters, and platform-fee cap.
- `cd contracts/compute_market && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `compute_market` fixed and validated.
- Continue Part 5 agent/storage/payments with `moss_storage`, then `sporepay` and `sporevault`.

### Part 5 Resume Point During Task 5.7 - After MossStorage

Checkpoint time: 2026-04-26 04:00:25 +0400

Current next investigation target:

- Continue Part 5 agent/storage/payments with `sporepay`, then `sporevault`.

MossStorage question:

- Can `moss_storage` lose storage payments, let unauthorized callers mutate provider/file state, or corrupt reputation/capacity/stat counters when escrow, expiry, staking, pointer input, challenge response, or slash arithmetic fails?

MossStorage provisional answer:

- Yes. The baseline suite was green, but LICN payouts treated any `Ok(_)` as success, `store_data()` could wrap counters or cap overflowed storage cost at `u64::MAX`, provider confirmation used saturating capacity addition that could admit overflow at max capacity, challenge and provider counters used unchecked increments, challenge responses read unbounded caller memory from committed size, and slash payout failures were ignored after stake reduction.

MossStorage evidence:

- `transfer_licn_out()` returned success for `Ok(false)` token/native transfer status.
- `store_data()` saturated an overflowed cost to `u64::MAX`, used unchecked `data_count + 1`, and accepted saturating expiry timestamps.
- `confirm_storage()` only checked the entry header length before decoding provider entries, used `used.saturating_add(data_size)` for capacity checks, and incremented stored count with `+ 1`.
- Most exported pointer entrypoints copied raw 32-byte pointers without null checks.
- `issue_challenge()` incremented `moss_challenge_count` with `+ 1`.
- `respond_challenge()` built a slice from `response_ptr` and the committed data size without null or upper-bound checks.
- `slash_provider()` computed `stake * slash_pct / 100` in `u64` and ignored failed challenger/treasury redistribution transfers after reducing provider stake.

MossStorage fix:

- Added null-safe address reads, safe stored-u64 helpers, saturating counter helpers, data-entry provider-length validation, LICN token loading, and unpaid LICN payout accounting.
- LICN outgoing transfers now require explicit `Ok(true)`.
- `store_data()` now rejects duplicate hashes before payment checks, rejects cost/count/expiry overflow, and writes `data_count` only after checks pass.
- `confirm_storage()` now rejects malformed provider lists, checks capacity with `checked_add`, bounds provider count against `MAX_PROVIDERS_PER_ENTRY`, and saturates stored counters.
- `claim_storage_rewards()` now preserves vesting and reward positions when transfer returns `Ok(false)`.
- `issue_challenge()` now validates data-entry provider bytes and saturates challenge count.
- `respond_challenge()` now rejects null response pointers and committed response sizes above `MAX_CHALLENGE_RESPONSE_BYTES`.
- `slash_provider()` now uses wide arithmetic for slash amount, caps corrupt slash percentages to 100, and records unpaid challenger/treasury payouts if redistribution transfer fails.

MossStorage validation result:

- Baseline `cd contracts/moss_storage && cargo test --release` passed before the patch: 27 tests.
- Final `cd contracts/moss_storage && cargo test --release` passed: 36 tests.
- Added focused regressions for false reward-transfer status preserving vesting, data-count overflow, storage-cost overflow, capacity-addition overflow, stored-count saturation, challenge-count saturation, wide slash arithmetic, failed slash-payout unpaid accounting, and null challenge-response rejection.
- `cd contracts/moss_storage && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `moss_storage` fixed and validated.
- Continue Part 5 agent/storage/payments with `sporepay`, then `sporevault`.

### Part 5 Resume Point During Task 5.7 - After SporePay

Checkpoint time: 2026-04-26 04:03:25 +0400

Current next investigation target:

- Continue Part 5 agent/storage/payments with `sporevault`.

SporePay question:

- Can `sporepay` lose payment escrow, release or refund incorrectly, accept spoofed caller inputs, or wrap stream/payment/stat counters under failed token/native transfers or malformed pointers?

SporePay provisional answer:

- Yes. The baseline suite was green, but stream creation accepted `Ok(false)` escrow status, stream-count overflow happened after escrow collection, withdrawals accepted `Ok(false)` payout status, cancel counters used `+ 1`, and cancel settlement could double-refund the sender if the sender refund succeeded but the recipient-due transfer failed.

SporePay evidence:

- `create_stream()` and `create_stream_with_cliff()` only checked `.is_err()` on `receive_token_or_native()` and incremented `stream_count` with `stream_id + 1` after escrow.
- `withdraw_from_stream()` used saturating withdrawn addition and only checked `.is_err()` for recipient payout.
- `cancel_stream()` used unchecked cancel-count increments in both legacy and escrow paths.
- `cancel_stream()` transferred refund to sender before recipient due; if the second transfer failed, the stream stayed uncancelled and retry could refund the sender again.

SporePay fix:

- Added safe stored-counter helpers, explicit escrow receive/transfer success helpers, checked stream-id allocation, and unpaid payout accounting.
- Stream creation now rejects stream-count overflow before escrow and requires explicit `Ok(true)` escrow status.
- Withdrawals now check withdrawn arithmetic and require explicit successful transfer before keeping withdrawn-state changes.
- Cancel count now saturates in legacy and escrow paths.
- Escrow cancel now records unpaid recipient payout and finalizes the cancel if the sender refund has already succeeded but recipient transfer fails; if no prior payout succeeded, recipient transfer failure leaves the stream active for retry.

SporePay validation result:

- Baseline `cd contracts/sporepay && cargo test --release` passed before the patch: 36 tests.
- Final `cd contracts/sporepay && cargo test --release` passed: 43 tests.
- Added focused regressions for false escrow status in normal and cliff stream creation, stream-count overflow before escrow, false withdrawal transfer preserving withdrawn amount, partial cancel recipient failure recording unpaid payout after sender refund, recipient failure without prior refund preserving stream state, and cancel-count saturation.
- `cd contracts/sporepay && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `sporepay` fixed and validated.
- Continue Part 5 agent/storage/payments with `sporevault`.

### Part 5 Resume Point After Task 5.7 - Agent/Storage/Payments Complete

Checkpoint time: 2026-04-26 04:09:38 +0400

Current next investigation target:

- Continue Part 5 privacy/bridge with `shielded_pool`, then `lichenbridge`.

SporeVault question:

- Can `sporevault` silently lose LICN payouts, accrue protocol fees before caller verification, corrupt share/asset/strategy accounting, or wrap harvest math under failed token transfers, malformed pointers, or extreme stored values?

SporeVault provisional answer:

- Yes. The baseline suite was green, but LICN transfer out treated `Ok(false)` as success, deposit accrued fees before the safe caller read/verification path was fully ordered, failed first deposits could leave fee state behind, withdrawals could fail to restore exact fee state, admin/view entrypoints had raw pointer reads, strategy allocation totals used unchecked addition, and harvest used `u64` multiplication/addition for deployed amount, total yield, and performance fee.

SporeVault evidence:

- `transfer_licn_out()` returned success for any `Ok(_)` token/native transfer result.
- Deposit/withdraw/admin/view entrypoints copied raw 32-byte pointers directly in multiple places.
- First-deposit setup wrote locked-share state before all checked accounting was complete, and fee accrual happened before later failure exits.
- Withdrawal protocol-fee rollback was not exact when existing fee storage was near `u64::MAX`.
- `add_strategy()` and `update_strategy_allocation()` could wrap total allocation checks with corrupt/extreme stored allocation values.
- `harvest()` computed `total_assets * allocation`, `total_yield += strategy_yield`, and `total_yield * PERFORMANCE_FEE_PERCENT` in `u64`.

SporeVault fix:

- Added null-safe 32-byte address reads and shared LICN token loading.
- LICN outbound transfers now require explicit `Ok(true)` and reject missing/malformed token config.
- Deposit now verifies the signer before fee/share/accounting mutation, validates configured fee bounds, uses checked fee/share/asset additions, and writes first-deposit locked shares only after all checks pass.
- Failed first deposits no longer accrue protocol fees.
- Withdrawals now reject invalid total-share accounting, validate withdrawal fee bounds, preserve exact previous protocol fees, and restore shares/assets/fees on false transfer status.
- Protocol-fee withdrawals preserve fee state on failed or false transfer status.
- Strategy allocation sums use checked addition.
- Harvest deployed/yield/performance-fee arithmetic now uses `u128` or saturating math.

SporeVault validation result:

- Baseline `cd contracts/sporevault && cargo test --release` passed before the patch: 51 tests.
- Final `cd contracts/sporevault && cargo test --release` passed: 59 tests.
- Added focused regressions for spoofed-caller deposit fee safety, failed first-deposit fee safety, false withdrawal transfer rollback with near-maximum fee state, false protocol-fee withdrawal preservation, add/update allocation overflow rejection, null user-position pointer rejection, and harvest wide arithmetic.
- `cd contracts/sporevault && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `sporevault` fixed and validated.
- Part 5 agent/storage/payments family is complete.
- Continue Part 5 privacy/bridge with `shielded_pool`, then `lichenbridge`.

### Part 5 Resume Point During Task 5.8 - After ShieldedPool

Checkpoint time: 2026-04-26 04:13:24 +0400

Current next investigation target:

- Continue Part 5 privacy/bridge with `lichenbridge`.

ShieldedPool question:

- Can `shielded_pool` preserve shielded value/nullifier/accounting invariants under malformed proofs, duplicate state, transfer-cap failures, corrupt stored state, unbounded payloads, and unsafe ABI pointers?

ShieldedPool provisional answer:

- No. The baseline suite was green, but zero-value operations were accepted, proof/encrypted-note/JSON buffers were unbounded, duplicate commitments and duplicate nullifiers were not rejected by the contract state layer, transfer capacity failures could mutate the in-memory pool before returning `CommitmentsFull`, shield pool-balance overflow could leave an inserted commitment in the pure state API, and the WASM ABI used null-unsafe pointer reads plus corrupt-state fallback to a fresh pool.

ShieldedPool evidence:

- `shield()` pushed commitments and incremented `commitment_count` before checking `pool_balance.checked_add(amount)`.
- `transfer()` inserted spent nullifiers before proving there was capacity for every output commitment.
- `transfer()` accepted duplicate input nullifiers and duplicate/new outputs that matched existing commitments.
- `shield()`, `unshield()`, and `transfer()` accepted zero-value or empty-state-changing requests so long as the proof payload was nonempty.
- `initialize()`, `check_nullifier()`, and JSON request entrypoints copied or sliced raw pointers without null/length guards.
- `load_state()` deserialized corrupt storage with `unwrap_or_else(|_| ShieldedPoolState::new())`, which could overwrite existing pool/nullifier state on the next successful save.

ShieldedPool fix:

- Added request/proof/encrypted-note/input/output bounds aligned with the 5 MiB transaction cap and a 64 KiB encrypted-note cap.
- Added shared null-safe ABI pointer/buffer readers.
- Added corrupt-state fail-closed loading and explicit state-save result checks.
- `shield()` now rejects zero amount, duplicate commitments, oversized notes/proofs, full commitment pools, and pool-balance overflow before mutating.
- `unshield()` now rejects zero amount, zero nullifier, zero recipient, oversized proofs, stale roots, spent nullifiers, and insufficient balance before mutating.
- `transfer()` now rejects empty/oversized input/output vectors, zero or duplicate nullifiers, duplicate/existing output commitments, oversized notes/proofs, and commitment-cap exhaustion before spending nullifiers or inserting outputs.
- Transfer Merkle-root recomputation now happens once after all preflighted outputs are inserted.

ShieldedPool validation result:

- Baseline `cd contracts/shielded_pool && cargo test --release` passed before the patch: 10 tests.
- Final `cd contracts/shielded_pool && cargo test --release` passed: 23 tests.
- Added focused regressions for zero shield amount, atomic shield pool-overflow failure, duplicate shield commitment rejection, encrypted-note size cap, zero unshield amount, duplicate transfer nullifiers, duplicate/existing output commitments, atomic transfer capacity failure, null ABI pointers, shield null-args guard cleanup, and corrupt stored state failing closed.
- `cd contracts/shielded_pool && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Status:

- `shielded_pool` fixed and validated.
- Continue Part 5 privacy/bridge with `lichenbridge`.

### Part 5 Closure - Contracts And Genesis Catalog Complete

Checkpoint time: 2026-04-26 04:18:38 +0400

Current next investigation target:

- Start Part 7 SDKs/CLI/developer portal contract review.

LichenBridge question:

- Can `lichenbridge` safely handle bridge escrow, mint, unlock, finalization, confirmation retry, duplicate messages, transfer-status failures, counter overflow, deadline overflow, and admin/query pointer inputs?

LichenBridge provisional answer:

- No. The baseline suite was green, but token/native payouts treated `Ok(false)` as success, final-threshold confirmation failures consumed the confirming validator and made exact-threshold retries impossible, lock and validator counters could overflow or saturate into bad accounting, nonce overflow was unchecked, expiry checks used saturating deadlines, and exported pointer entrypoints copied raw 32-byte pointers without null guards.

LichenBridge evidence:

- `transfer_out()` only checked `Err(_)`, so `Ok(false)` token/native transfer status completed mint/unlock flows.
- `confirm_mint()` and `confirm_unlock()` wrote the validator confirmation before attempting the final payout; if payout failed, the same validator could not retry and exact-threshold requests could remain pending permanently.
- `allocate_nonce()` stored `nonce + 1` without overflow checks.
- `lock_tokens()` used `locked.saturating_add(amount)`, which can under-account locked bridge reserves at `u64::MAX`.
- `add_bridge_validator()` wrote validator state before an unchecked `validator_count + 1`.
- Expiry checks used `created_slot.saturating_add(timeout)`, so corrupt near-`u64::MAX` created slots could avoid expiry.
- Admin, validator, query, lock, mint, unlock, token, identity, and pause entrypoints directly dereferenced raw address pointers.

LichenBridge fix:

- Added a shared null-safe 32-byte address reader and applied it across exported pointer entrypoints.
- `transfer_out()` now requires explicit `Ok(true)` and returns transfer failure on `Ok(false)`.
- Mint/unlock final confirmation now attempts payout before recording the confirming validator when the threshold would be reached, so failed payouts do not consume retry authority.
- Non-final confirmations still record immediately.
- Lock amount, nonce allocation, validator count, and expired-unlock reserve restoration use checked arithmetic.
- Lock now rejects zero destination-chain hashes as well as zero destination addresses.
- Deadline overflow is treated as expired through a checked deadline helper.

LichenBridge validation result:

- Baseline `cd contracts/lichenbridge && cargo test --release` passed before the patch: 53 tests.
- Final `cd contracts/lichenbridge && cargo test --release` passed: 60 tests.
- Added focused regressions for false mint-transfer status preserving retry, false unlock-transfer status preserving retry and reserved balance, locked-amount overflow before nonce allocation, nonce overflow before lock state writes, validator-count overflow atomicity, expiry-deadline overflow, and null pointer entrypoints.
- `cd contracts/lichenbridge && cargo fmt -- --check` passed.
- `cargo fmt --all -- --check` passed.
- `git diff --check` passed.
- `python3 scripts/qa/update-expected-contracts.py --check` passed.

Part 5 closure status:

- All contract families in the plan have been audited with scoped release tests and root hygiene gates.
- The 29 in-tree contract directories remain reconciled with the 28-contract genesis catalog; `mt20_token` remains the documented non-genesis contract.
- Continue with Part 7 SDKs/CLI/developer portal contract review.

### Part 7 Closure - SDKs, CLI, And Developer Portal

Checkpoint time: 2026-04-26 04:28:35 +0400

Part 7 question:

- Do CLI, SDKs, and developer docs match current transaction format, package names, RPC surface, live-test defaults, and package dependencies?

Part 7 answer:

- Code-level SDK/CLI validation is green.
- The docs needed drift fixes: Rust SDK package/library names, actual Rust example names, current wire-envelope submission helper, Python/JS fixed endpoint-count claims, Python dependency names, and RPC examples showing stale `0.4.8`/mainnet metadata.

Part 7 fixes:

- `sdk/python/deep_stress_test.py`: helper named `test` is no longer collected as a pytest test.
- `sdk/python/test_sdk_live.py`, `sdk/python/test_websocket_sdk.py`, and `sdk/python/test_websocket_simple.py`: live local-validator scripts remain directly runnable but are skipped by default under pytest unless `LICHEN_RUN_LIVE_SDK_TESTS=1`.
- `sdk/python/lichen/connection.py`: stale "24 RPC endpoints" docstring replaced with endpoint-family wording.
- `developers/sdk-python.html`: Python requirement/dependencies now match `pyproject.toml` (`Python 3.9+`, `httpx`, `websockets`, `base58`, `cryptography`, `dilithium-py`) and mainnet wording no longer implies the public testnet examples are mainnet.
- `developers/rpc-reference.html`: endpoint copy now presents public testnet first, mainnet as the mainnet target, and chain-status sample uses `lichen-testnet-1` / `testnet`.
- Ignored local docs patched and recorded: `docs/api/RUST_SDK.md` now uses `lichen-client-sdk` / `lichen_client_sdk`, actual Rust examples, and `Client::send_transaction`; `docs/api/PYTHON_SDK.md` no longer links missing `docs/SDK.md` or claims 24 endpoints; `docs/api/JAVASCRIPT_SDK.md` no longer claims 24 endpoints and mentions `@noble/post-quantum`; `docs/guides/RPC_API_REFERENCE.md` now uses version `0.5.9`, public testnet endpoint, and testnet metadata examples.

Part 7 validation result:

- `cargo check -p lichen-cli --tests` passed.
- `cargo test -p lichen-cli -- --nocapture` passed.
- `cd sdk/rust && cargo check --tests && cargo test -- --nocapture` passed.
- `cd sdk/js && npm run test && npm run build && node test_cross_sdk_compat.js` passed.
- `cd sdk/python && ./venv/bin/python -m pytest -q` passed with 108 passed and 3 skipped.
- Stale-string scan over SDK/API/developer docs for `0.4.8`, `lichen-mainnet`, `24 endpoints`, `PyNaCl`, missing `docs/SDK.md`, `lichen_sdk`, and nonexistent Rust examples returned no hits in the targeted files.
- `git diff --check` passed.

Status:

- Part 7 is complete.
- Continue with Part 8 frontends and user-facing wiring.

### Part 8 Slice - Frontend RPC Parity Cleanup

Checkpoint time: 2026-04-26 04:32:51 +0400

Part 8 question:

- Does every live frontend RPC action map to a supported server method or present an honest unavailable state?

Part 8 provisional answer:

- No at baseline. Static frontend tests passed, but `npm run audit-frontend-rpc-parity` still found 7 unknown live RPC calls: `getShieldedNotes`, `sendShieldedTransaction`, `submitProgramVerification`, `submitShieldedTransfer`, `submitShieldTransaction`, and `submitUnshieldTransaction`.

Part 8 fix:

- `wallet/js/shielded.js`: unsigned shield/unshield/private-transfer submit paths now explicitly mark signed shielded submission unavailable instead of calling missing RPC methods. Modal buttons are disabled with an honest title until the signed shielded transaction builder exists.
- `wallet/extension/src/pages/full.js`: removed `getShieldedNotes` and `sendShieldedTransaction` RPC calls; extension shielded notes now come from local state and submit buttons are disabled until signed shielded transaction submission is implemented.
- `wallet/extension/src/popup/popup.js`: removed unsupported `getShieldedNotes`; popup balance is derived from local cached shielded notes.
- `programs/js/lichen-sdk.js`: program verification no longer calls missing `submitProgramVerification`; it returns a local queued result.
- `scripts/qa/test_wallet_audit.js` and `scripts/qa/test_wallet_extension_audit.js`: updated regressions to enforce that unsupported shielded RPC calls are not reintroduced.

Part 8 validation result:

- `npm run test-frontend-assets` passed: 244 asset checks and 2 shared-helper checks.
- `npm run test-wallet` passed: 91 wallet audit checks.
- `npm run test-wallet-extension` passed: 86 extension audit checks.
- `npm run audit-frontend-rpc-parity` passed with 0 unknown live RPC calls; 25 dynamic/manual calls remain listed for owner review.
- `node --check monitoring/js/monitoring.js`, `wallet/js/wallet.js`, `dex/dex.js`, and `explorer/js/explorer.js` passed.
- Focused syntax checks for `wallet/js/shielded.js`, `wallet/extension/src/pages/full.js`, `wallet/extension/src/popup/popup.js`, `programs/js/lichen-sdk.js`, and the two updated QA scripts passed.
- `git diff --check` passed.

Status:

- Part 8 RPC/frontend parity slice is complete.
- Continue Part 8 broader frontend endpoint/network selector, dead-link, stale package-name, and honest-disabled-state review.

### Part 8 Slice - Developer Portal Version Drift Cleanup

Checkpoint time: 2026-04-26 04:36:04 +0400

Part 8 question:

- Are frontend/developer-facing pages still carrying stale release/version-count wording after the SDK and RPC parity cleanup?

Part 8 provisional answer:

- Yes. A targeted scan found stale `v0.5.7` / `lichen 0.5.7` text in the tracked developer portal pages even though the active release line is `v0.5.9`.

Part 8 fix:

- `developers/rpc-reference.html`: the implementation-verified surface callout now says `v0.5.9`, reports the current 182 JSON-RPC dispatch-name surface across native/Solana/EVM dispatch, and removes the stale exact `72 REST endpoints` count in favor of REST route-family wording.
- `developers/cli-reference.html`: CLI surface coverage and version output examples now use `v0.5.9` / `lichen 0.5.9`.
- `developers/getting-started.html`: the CLI version smoke-test output now uses `lichen 0.5.9`.

Part 8 validation result:

- `rg -n "v0\\.5\\.7|lichen 0\\.5\\.7|0\\.5\\.7" developers website wallet explorer dex marketplace programs monitoring faucet -g '*.html' -g '*.js'` returned no matches.
- `npm run test-frontend-assets` passed: 244 asset checks and 2 shared-helper checks.
- `git diff --check` passed.

Status:

- Part 8 developer portal version drift slice is complete.
- Continue Part 8 broader frontend endpoint/network selector, dead-link, stale package-name, and honest-disabled-state review.

### Part 8 Slice - Endpoint And Copy Honesty Cleanup

Checkpoint time: 2026-04-26 04:38:14 +0400

Part 8 question:

- Are public frontend/developer pages still using stale mainnet-switch wording or unsupported production/uptime claims?

Part 8 provisional answer:

- Yes. The remaining actionable scan hits were small copy issues: SDK/WebSocket/website examples said "for mainnet" instead of matching the public-testnet/mainnet-target convention, and the programs portal claimed "29 Production-Ready Contracts" plus "99.9% uptime" even though the verified repo fact is 28 genesis contracts plus the standalone in-tree MT-20 template.

Part 8 fix:

- `developers/ws-reference.html`: WebSocket endpoints now list public testnet first and mainnet as a target; JS/Python examples say to switch only when targeting mainnet.
- `developers/sdk-js.html`, `developers/sdk-rust.html`, and `website/index.html`: examples now keep public testnet as the default and frame mainnet endpoints as opt-in targets.
- `programs/index.html`: visible copy now says "28 Genesis Contracts + MT-20 Template" and "Live Testnet Tooling" instead of unsupported production/uptime claims.
- `programs/js/playground-complete.js`: header comment no longer says "Actually Production Ready".

Part 8 validation result:

- Focused endpoint/claim scan returned no matches for stale "switch for mainnet", old WebSocket labels, `29 Production-Ready`, `99.9% uptime`, or `Production Ready` in first-party frontend/developer files.
- `npm run test-frontend-assets` passed after the HTML copy edits: 244 asset checks and 2 shared-helper checks.
- `node --check programs/js/playground-complete.js` passed.
- `git diff --check` passed.

Status:

- Part 8 endpoint/copy honesty slice is complete.
- Continue Part 8 with any remaining first-party frontend link/selector/dead-state checks, then close Part 8 if no new actionable drift remains.

### Part 8 Static Closure - Frontends And User-Facing Wiring

Checkpoint time: 2026-04-26 04:39:10 +0400

Part 8 question:

- After the RPC parity, developer portal drift, endpoint wording, and copy honesty fixes, is the static frontend/user-facing slice ready to close?

Part 8 answer:

- Yes for static validation. The remaining planned Part 8 item is the manual local-stack validation slice.

Part 8 static closure validation result:

- `npm run test-frontend-assets` passed: 244 asset checks and 2 shared-helper checks.
- `npm run test-wallet` passed: 91 checks.
- `npm run test-wallet-extension` passed: 86 checks.
- `npm run audit-frontend-rpc-parity` passed with 0 unknown live RPC calls; 25 dynamic/manual calls remain listed for owner review.
- Previous focused syntax gates passed for `monitoring/js/monitoring.js`, `wallet/js/wallet.js`, `dex/dex.js`, `explorer/js/explorer.js`, touched wallet/extension/program files, and `programs/js/playground-complete.js`.
- Stale developer-page scan for `v0.5.7` / `lichen 0.5.7` / `0.5.7` returned no matches.
- Endpoint/claim scan returned no matches for stale "switch for mainnet", old WebSocket labels, `29 Production-Ready`, `99.9% uptime`, or `Production Ready` in first-party frontend/developer files.
- `git diff --check` passed.

Status:

- Part 8 static frontend/user-facing closure is complete.
- Next Part 8 action: run or explicitly defer the manual local-stack checks from the plan, then move to Part 9 supply chain, CI, release, and packaging.

### Part 8 Manual Local-Stack Validation - Harness Hardening

Checkpoint time: 2026-04-26 04:50:26 +0400

Part 8 question:

- Can the manual local-stack validation be trusted to fail when the RPC server is unavailable, and is it probing the current REST route surface?

Part 8 provisional answer:

- Not at baseline. A standalone `./scripts/start-local-stack.sh testnet` printed ready, but validator/custody/faucet child processes were gone after the command returned in the tool environment. The first `node scripts/qa/e2e-rpc-coverage.js` run then falsely passed many probes because transport failures and invalid/non-JSON REST responses were treated as acceptable method-level outcomes. The REST endpoint list also used stale unprefixed paths such as `/pairs` and `/shielded/pool`.

Part 8 fix:

- `scripts/qa/e2e-rpc-coverage.js`: JSON-RPC, Solana-compatible, EVM-compatible, and REST helpers now fail transport/protocol errors and invalid JSON instead of counting them as wiring success.
- REST probes now use the actual local server route prefixes and paths under `/api/v1`, `/api/v1/prediction-market`, `/api/v1/launchpad`, and `/api/v1/shielded`.

Part 8 validation result:

- `node --check scripts/qa/e2e-rpc-coverage.js` passed.
- `RPC_URL=http://127.0.0.1:1 node scripts/qa/e2e-rpc-coverage.js` failed as intended with `RPC Coverage: 0 passed, 129 failed, 0 skipped`.
- `LICN_LOCAL_NETWORK=testnet ./scripts/start-local-3validators.sh status` reports `status=down reachable_rpc=0/3`, and `pgrep -fl 'lichen-validator|lichen-custody|lichen-faucet|run-validator'` returned no stale processes before the rerun.

Status:

- Harness hardening is complete.
- Next action: rerun the manual Part 8 stack slice in one trapped shell so services remain alive through `node scripts/qa/e2e-rpc-coverage.js`, `bash scripts/qa/test-rpc-comprehensive.sh`, and `bash scripts/qa/test-cli-comprehensive.sh`.

### Part 8 Manual Local-Stack Validation - Candle Probe Correction

Checkpoint time: 2026-04-26 04:52:10 +0400

Part 8 question:

- Does the hardened live-stack coverage pass after switching to real `/api/v1` REST paths?

Part 8 provisional answer:

- Not yet. The first trapped live-stack rerun kept services alive and proved the harness is now failing real REST incompatibilities: `node scripts/qa/e2e-rpc-coverage.js` reported `RPC Coverage: 128 passed, 1 failed, 0 skipped`.

Part 8 finding/fix:

- The single failure was `REST /api/v1/pairs/1/candles?interval=1m&limit=5`, which returned a plain text query deserialization error because `rpc/src/dex.rs` defines `CandleQuery.interval` as `Option<u64>`. The coverage probe now uses `interval=60&limit=5`.

Part 8 validation result:

- `node --check scripts/qa/e2e-rpc-coverage.js` passed after the probe correction.
- The failed live-stack run exited through the cleanup trap; `LICN_LOCAL_NETWORK=testnet ./scripts/start-local-3validators.sh status` reported `status=down reachable_rpc=0/3`.

Status:

- Probe correction is complete.
- Next action: rerun the trapped local-stack validation command and continue to the RPC comprehensive wrapper and CLI comprehensive script if coverage passes.

### Part 8 Manual Local-Stack Validation - Final Pass

Checkpoint time: 2026-04-26 04:53:44 +0400

Part 8 question:

- Does the full manual local-stack slice pass after hardening the harness and correcting the candle probe?

Part 8 answer:

- Yes. The stack must be kept in the same parent shell under this command runner, but the final trapped run passed RPC coverage, the RPC comprehensive wrapper, and the CLI comprehensive suite.

Part 8 validation result:

- Trapped local stack command started `./scripts/start-local-stack.sh testnet` with `LICHEN_KEYPAIR_PASSWORD=local-e2e-secret` and `RPC_URL=http://127.0.0.1:8899`.
- Direct `node scripts/qa/e2e-rpc-coverage.js` passed with `RPC Coverage: 129 passed, 0 failed, 0 skipped`.
- `bash scripts/qa/test-rpc-comprehensive.sh` passed by rerunning the same coverage with `RPC Coverage: 129 passed, 0 failed, 0 skipped`.
- `bash scripts/qa/test-cli-comprehensive.sh` passed with `PASSED: 28`, `FAILED: 0`, `SKIPPED: 0`; environment-limited transfer/staking/contract-list subflows were reported as expected passes by the script.
- Cleanup trap ran after success. `LICN_LOCAL_NETWORK=testnet ./scripts/start-local-3validators.sh status` reported `status=down reachable_rpc=0/3`, and `pgrep -fl 'lichen-validator|lichen-custody|lichen-faucet|run-validator'` returned no stale processes.
- `git diff --check` passed after the Part 8 manual-validation patches and checkpoint updates.

Status:

- Part 8 frontend/user-facing and manual local-stack validation is complete.
- Next action: start Part 9 supply chain, CI, release, and packaging.

## Part 9 - Supply Chain, CI, Release, And Packaging

### Part 9 Slice - Dependency Health And Release Provenance

Checkpoint time: 2026-04-26 04:58:25 +0400

Part 9 question:

- Does CI currently represent JS/Python dependency checks and reproducible npm installs, and do release artifacts have machine-verifiable provenance?

Part 9 provisional answer:

- Partially at baseline. Rust posture was already represented through all-lockfile `cargo audit`, `cargo deny`, workspace/contract tests, WASM builds, and Rust SBOM generation. The gaps were that npm lockfiles were ignored, CI used `npm install` instead of lockfile installs, there was no JS/Python dependency-health job, and release artifacts had checksums/manual PQ signature guidance but no GitHub artifact attestations.

Part 9 fix:

- `.gitignore`: stopped ignoring `package-lock.json` so root and JS SDK npm lockfiles can be tracked.
- `package-lock.json` and `sdk/js/package-lock.json`: now visible as untracked files for reproducible root and JS SDK installs.
- `.github/workflows/ci.yml`: added `JS and Python Dependency Health` with root/SDK `npm ci --ignore-scripts`, `npm audit --omit=dev`, and Python SDK install plus `pip check`; changed existing CI npm installs to `npm ci --ignore-scripts`.
- `.github/workflows/release.yml`: added `id-token`, `attestations`, and `artifact-metadata` permissions; added `actions/attest@v4` provenance attestations for release archives and `SHA256SUMS`; added a GitHub CLI attestation verification command to release notes.

Part 9 validation result:

- `cargo deny check --config deny.toml advisories licenses sources` passed.
- All-lockfile audit loop passed over 34 Cargo.lock files with `cargo audit -q -D warnings --file <lockfile>`.
- `npm ci --ignore-scripts && npm audit --omit=dev` passed at repo root with 0 vulnerabilities.
- `cd sdk/js && npm ci --ignore-scripts && npm audit --omit=dev && npm run build` passed with 0 production vulnerabilities and a successful TypeScript build.
- `cd sdk/python && ./venv/bin/python -m pip check` passed.
- Isolated CI-style Python venv check passed: `python -m pip install -e ./sdk/python` then `python -m pip check`.
- `npm run test-wallet-extension && npm run test-wallet` passed: 86 extension checks and 91 wallet checks.
- Ruby YAML parse passed for `.github/workflows/ci.yml`, `.github/workflows/release.yml`, and `.github/workflows/wallet-extension-release.yml`.
- `git diff --check` passed.
- Note: direct system `python3 -m pip check` on local Homebrew Python 3.14 fails because global `wheel 0.46.3` lacks `packaging`; the isolated venv check shows this is not a repo dependency conflict.

Status:

- Dependency-health and release-provenance CI slice is complete.
- Next action: continue Part 9 review for remaining Docker, Scorecard, SBOM-attestation, release-documentation, and package-lock tracking details before closing Part 9.

### Part 9 Slice - Docker Packaging, Scorecard, And Public Guidance

Checkpoint time: 2026-04-26 05:01:55 +0400

Part 9 question:

- Are the remaining supply-chain/release surfaces aligned with current binaries, health methods, Scorecard posture, and operator verification guidance?

Part 9 answer:

- Yes after the final patches. The remaining baseline gaps were stale `infra` Docker packaging and missing OpenSSF Scorecard automation. Active README release guidance also lagged the new attestation posture.

Part 9 fix:

- `.github/workflows/scorecard.yml`: added a dedicated OpenSSF Scorecard workflow on `main` pushes and a weekly schedule, using `ossf/scorecard-action@v2.4.3`, SARIF output, and published results with job-scoped `id-token: write`.
- `infra/Dockerfile.lichen`: updated to Rust `1-bookworm`, build the current `lichen-validator` binary, use current runtime dependencies, call `getHealth`, and run `lichen-validator` instead of the non-existent standalone `lichen-rpc` binary.
- `infra/Dockerfile.custody`: updated to Rust `1-bookworm`, build the current `lichen-custody` binary from the workspace, and health-check/run `lichen-custody` instead of stale `custody-bridge`.
- `infra/Dockerfile.market-maker`: changed npm install to `npm install --ignore-scripts`.
- `infra/docker-compose.yml`: health checks now call `getHealth` and look for `lichen-custody`; market-maker build context now points at the repo root so the Dockerfile can copy the SDK sources it references.
- `README.md`: security highlights now list npm/Python dependency checks, OpenSSF Scorecard, and release provenance attestations; Linux/macOS release install examples now include `gh attestation verify`.
- `memories/repo/current-state.md`: updated durable CI/release supply-chain fact.

Part 9 validation result:

- `cargo check -p lichen-validator -p lichen-custody` passed.
- `GRAFANA_PASSWORD=dummy docker compose -f docker-compose.yml config` passed; Docker warned that the compose `version` key is obsolete.
- `GRAFANA_PASSWORD=dummy docker compose -f infra/docker-compose.yml config` passed; Docker warned that the compose `version` key is obsolete.
- Stale Docker string scan found no live hits for `method":"health`, `custody-bridge`, `lichen-rpc`, or `rust:1.75` in `Dockerfile`, `infra`, `.github`, `.gitignore`, and `README.md`.
- Ruby YAML parse passed for `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `.github/workflows/scorecard.yml`, and `.github/workflows/wallet-extension-release.yml`.
- `git diff --check` passed.

Status:

- Part 9 supply chain, CI, release, and packaging is complete.
- Next action: start Part 10 documentation, public claims, and mainnet decision ledger.

## Part 10 - Documentation, Public Claims, And Decision Ledger

### Part 10 Slice - Active Public Claim Cleanup

Checkpoint time: 2026-04-26 05:05:38 +0400

Part 10 question:

- Do active public docs and frontend surfaces still carry stale version, mainnet, production, or phantom-feature claims after Parts 7-9?

Part 10 provisional answer:

- Mostly no. Broad scans still hit expected historical docs, local-development examples, and old changelog/audit/strategy context, but the actionable active public hits were marketplace copy that still claimed "instant finality."

Part 10 fix:

- `marketplace/index.html`, `marketplace/browse.html`, `marketplace/create.html`, `marketplace/item.html`, and `marketplace/profile.html`: replaced "instant finality" / "Instant finality" with "fast BFT commitment" / "Fast BFT commitment."
- Local ignored internal `DEPLOYMENT_STATUS.md`: added a read-me-first note clarifying that the file is a deployment ledger, not a single current checklist; older phase/session rows are preserved for provenance and may contain historical TODOs or pre-v0.5.x state.
- Local ignored internal `docs/strategy/BLOCKCHAIN_PUBLIC_CLAIM_CORRECTIONS.md`: added a status note clarifying that it is a historical correction ledger and not automatically an active TODO list without rechecking referenced files.

Part 10 validation result:

- Targeted active public scan for `instant finality`, `32-slot`, `mainnet ready`, `mainnet-ready`, `production ready`, `Production Ready`, `v0.5.7`, `v0.5.8`, `v0.5.6`, `0.4.8`, and `not wired` returned no actionable matches after the marketplace patch.
- `npm run test-frontend-assets` passed after the marketplace copy edits: 244 asset checks and 2 shared-helper checks.
- `git diff --check` passed after Part 10 marketplace/docs/current-state patches.
- `memories/repo/current-state.md` now records the active public/developer stale-claim cleanup and keeps older deployment/audit/changelog/strategy documents classified as historical unless rechecked. The directly edited `DEPLOYMENT_STATUS.md` and `docs/strategy/BLOCKCHAIN_PUBLIC_CLAIM_CORRECTIONS.md` copies are ignored internal docs and were not force-added to the public commit.

Status:

- Part 10 documentation, public claims, and classification cleanup is complete.
- Next action: create the final mainnet-ready exit-criteria ledger from the plan evidence, or stop with a final response if no further ledger is needed.

## Final Mainnet-Ready Exit-Criteria Ledger

Checkpoint time: 2026-04-26 05:06:15 +0400

Decision:

- The scoped repo audit supports a mainnet-candidate decision for the current workspace changes, subject to committing the new/untracked release artifacts and letting the normal CI/release pipeline run as the final gate.
- This audit did not touch live VPS or deployment state. Operational launch approval still needs date-aware reconciliation against live deployment records.

Exit criteria:

1. No Critical or High findings remain open: satisfied by the tracker evidence. Critical/high findings discovered during the pass were fixed with focused regressions; no open Critical/High item is recorded after Part 10.
2. Medium findings are fixed, explicitly non-blocking, or moved to context: satisfied for this pass. Residual notes are non-blocking: the LichenSwap patch intentionally avoided a broad legacy AMM rewrite, local Homebrew `pip check` is an environment issue, Docker Compose only warns about obsolete `version`, and `cd sdk && cargo fmt -- --check` still sees pre-existing rustfmt drift in untouched `sdk/src/dex.rs` and `sdk/src/nft.rs` while changed-file/root formatting gates passed.
3. Module-scoped Rust checks pass for every workspace crate: satisfied by the recorded scoped checks/tests for core, validator, RPC, CLI, P2P, faucet, custody, and genesis-related paths. A single `cargo check --workspace` was intentionally not used as the iteration gate.
4. Contract-local check/test/WASM posture is covered for genesis contracts and `mt20_token`: satisfied by Part 5 contract-family release-test closures plus repeated `python3 scripts/qa/update-expected-contracts.py --check`, with `mt20_token` documented as the known non-genesis template.
5. Frontend asset, shared-helper, wallet, and extension audits pass: satisfied. Part 8 records `npm run test-frontend-assets`, `npm run test-wallet`, and `npm run test-wallet-extension` passing, and Part 10 reran `npm run test-frontend-assets` after public-copy edits.
6. JS, Rust, and Python SDK serialization/golden-vector checks pass: satisfied. Part 7 records Rust SDK checks/tests, JS SDK tests/build/cross-SDK vector, and Python SDK pytest with live-validator scripts opt-in.
7. RPC/frontend/developer portal parity has no unexplained live-action gaps: satisfied. `npm run audit-frontend-rpc-parity` passed with 0 unknown live RPC calls after unsupported frontend flows were made local/disabled.
8. Local production-parity stack runs and passes RPC/CLI E2E: satisfied. The trapped local testnet run passed direct RPC coverage, the RPC comprehensive wrapper, and CLI comprehensive checks, then cleaned up all local services.
9. CI/release/supply-chain posture has no unexplained lockfile, advisory, signing, or provenance gaps: satisfied once the new/untracked artifacts are included in the commit. Part 9 added npm lockfile tracking, JS/Python dependency health, release attestations, Scorecard, Docker packaging fixes, and README verification guidance.
10. Public docs and developer portal claims match code and live testnet reality: satisfied for active/public surfaces. Historical deployment, audit, changelog, and strategy documents remain provenance context unless rechecked.

Final validation:

- `git diff --check` passed after the final handover/tracker/current-state updates and this ledger.

Status:

- Repo-wide mainnet-readiness audit pass is complete.
- Final response should call out the conditional nature of the decision: commit the untracked lockfiles/workflow/docs, run CI/release gates, and reconcile live deployment state before an actual mainnet launch.
