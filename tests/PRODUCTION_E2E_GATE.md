# Production E2E Gate

`tests/production-e2e-gate.sh` is the launch-gate runner for end-to-end verification across wallet, RPC, WebSocket, contracts, CLI, and LichenID read-path behavior.

## What it does

1. Validates local tooling (`curl`, `jq`, `python3`) and builds `lichen` if needed.
2. Verifies chain health (primary RPC and optional multi-validator health).
3. Creates two real actor wallets:
   - `e2e-agent`
   - `e2e-human`
4. Funds each wallet from treasury with `1000 LICN` (configurable).
5. Executes a real transfer between actor wallets.
6. Runs integrated suites:
   - `tests/test-rpc-comprehensive.sh`
   - `tests/test-websocket.sh`
   - `tests/live-e2e-test.sh`
   - `tests/services-deep-e2e.sh`
   - `tests/contracts-write-e2e.py`
   - `tests/test-contract-deployment.sh`
   - `tests/test-cli-comprehensive.sh`
   - optional: `scripts/test-all-sdks.sh`
7. Fails the gate on any failing stage.

## Usage

```bash
cd /Users/johnrobin/.openclaw/workspace/moltchain
bash tests/production-e2e-gate.sh
```

## Important environment flags

- `RPC_URL` (default `http://localhost:8899`)
- `WS_URL` (default `ws://localhost:8900`)
- `TREASURY_KEYPAIR` (default `data/state-testnet-7001/genesis-keys/treasury-lichen-testnet-1.json`)
- `TREASURY_FUND_LICN` (default `1000`)
- `REQUIRE_MULTI_VALIDATOR` (default `1`)
- `STRICT_NO_SKIPS` (default `1`)
- `RUN_SDK_SUITE` (default `0`)
- `RUN_DEEP_SERVICES_SUITE` (default `1`)
- `RUN_CONTRACT_WRITE_SUITE` (default `1`)
- `AGENT_WALLET_NAME` / `HUMAN_WALLET_NAME`
- `REQUIRE_DEX_API` (default `1`)
- `REQUIRE_FAUCET` (default `1`)
- `REQUIRE_CUSTODY` (default `1`)
- `REQUIRE_LAUNCHPAD` (default `1`)
- `REQUIRE_TOKEN_WRITE` (default `1`)
- `REQUIRE_ALL_CONTRACTS` (default `1`)
- `REQUIRE_ALL_SCENARIOS` (default `1`)
- `STRICT_WRITE_ASSERTIONS` (default `1`)
- `TX_CONFIRM_TIMEOUT_SECS` (default `25`)
- `REQUIRE_FULL_WRITE_ACTIVITY` (default `1`)
- `MIN_CONTRACT_ACTIVITY_DELTA` (default `1`)
- `CONTRACT_ACTIVITY_OVERRIDES` (default `{"dex_core":7,"dex_router":4,"dex_margin":6,"lichenbridge":3,"thalllend":4,"lichenswap":4,"lichenoracle":4,"lichenpunks":4,"moss_storage":3,"sporepump":3,"prediction_market":3,"lichenid":8}`)
- `ENFORCE_DOMAIN_ASSERTIONS` (default `1`)
- `ENABLE_NEGATIVE_ASSERTIONS` (default `1`)
- `REQUIRE_NEGATIVE_REASON_MATCH` (default `1`)
- `REQUIRE_NEGATIVE_CODE_MATCH` (default `0`)
- `REQUIRE_SCENARIO_FOR_DISCOVERED` (default `1`)
- `MIN_NEGATIVE_ASSERTIONS_EXECUTED` (default `5`)
- `REQUIRE_EXPECTED_CONTRACT_SET` (default `1`)
- `EXPECTED_CONTRACTS_FILE` (default `tests/expected-contracts.json`)
- `WRITE_E2E_REPORT_PATH` (default `tests/artifacts/contracts-write-e2e-report.json`)
- `DEX_BOOTSTRAP_BASE_SYMBOL` (default `LICN`)
- `DEX_BOOTSTRAP_QUOTE_SYMBOL` (default `LUSD`)
- `DEX_API_URL` (default `${RPC_URL}/api/v1`)
- `FAUCET_URL` (default `http://localhost:9100`)
- `CUSTODY_URL` (default `http://localhost:9105`)

Example (single-validator local dev):

```bash
REQUIRE_MULTI_VALIDATOR=0 STRICT_NO_SKIPS=0 bash tests/production-e2e-gate.sh
```

## Current scope notes

- This gate validates real actor lifecycle for wallet + treasury funding + transfer.
- Deep services coverage now includes token lifecycle writes, launchpad contract discoverability/stats checks, DEX API, faucet, custody, contract/program event surfaces.
- Contract-by-contract enforcement is enabled: every contract directory under `contracts/` must be discoverable in deployed contract inventory and pass generic program/contract endpoint checks.
- DEX bootstrap pair assertion is included (`LICN/lUSD` by default, configurable via env vars).
- Contract write-scenario coverage is included via `tests/contracts-write-e2e.py` for real state-changing actions across protocol contracts, including DEX modules and non-DEX domains (`lichenid`, `bountyboard`, `lichenpunks`, `lichenoracle`, `lichenswap`, `lusd_token`, `weth_token`, `wsol_token`).
- Write scenarios now enforce transaction confirmation and post-write observability deltas (`getProgramCalls`/`getContractEvents`) for mutating actions when `STRICT_WRITE_ASSERTIONS=1`.
- Contract-level aggregate activity floors are enforced after each contract scenario block; by default, required delta is at least the number of mutating scenario steps (`REQUIRE_FULL_WRITE_ACTIVITY=1`).
- Critical contracts have explicit default activity thresholds in the gate via `CONTRACT_ACTIVITY_OVERRIDES`.
- Domain-specific post-state assertions are launch-blocking when `ENFORCE_DOMAIN_ASSERTIONS=1` (token lifecycle, launchpad/trading/lending/bridge/storage flows, NFT ownership transitions, oracle freshness, swap reserve movement, governance/action lifecycle flows).
- Guardrail/adversarial checks run when `ENABLE_NEGATIVE_ASSERTIONS=1` and require no unexpected state mutation for unauthorized or duplicate operations.
- Negative guardrail checks also enforce expected rejection reason markers from transaction payload when `REQUIRE_NEGATIVE_REASON_MATCH=1`.
- Optional exact rejection-code matching is available for negative checks when `REQUIRE_NEGATIVE_CODE_MATCH=1`.
- Scenario coverage can be enforced against discovered contracts with `REQUIRE_SCENARIO_FOR_DISCOVERED=1`.
- A minimum adversarial depth can be enforced with `MIN_NEGATIVE_ASSERTIONS_EXECUTED`.
- Expected deployed contract set can be lockfile-enforced with `REQUIRE_EXPECTED_CONTRACT_SET=1` and `EXPECTED_CONTRACTS_FILE`.
- Regenerate lockfile deterministically with `python3 tests/update-expected-contracts.py --write` (preview-only mode without `--write`).
- Validate lockfile parity locally with `make check-expected-contracts` (or `python3 tests/update-expected-contracts.py --check`).
- CI now enforces this lockfile parity on every push/PR via `.github/workflows/ci.yml`.
- Contract write runner emits a machine-readable JSON artifact with per-step outcomes and diagnostics for live triage.
- LichenID read-path coverage is included via `tests/live-e2e-test.sh` Section 21.
- Remaining full write-path coverage for every contract action still requires per-contract deterministic actor workflows (tracked in matrix).
