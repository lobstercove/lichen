# Production E2E Coverage Matrix

This matrix is used with `tests/production-e2e-gate.sh` to track launch-blocking coverage.

Execution order, suite ownership, and recent-runtime regression targets now live in `tests/PRODUCTION_E2E_EXPANSION_PLAN.md`. Treat that plan as the implementation tracker for closing the gaps below.

## Current automated coverage

- Wallet lifecycle
  - create agent wallet
  - create human wallet
  - treasury funding (1000 LICN configurable)
  - actor-to-actor transfer
- Core services
  - RPC comprehensive suite
  - WebSocket suite
  - user services suite (`tests/e2e-user-services.py`)
  - custody withdrawal burn-verification suite (`tests/e2e-custody-withdrawal.py`)
  - portal interaction runtime suite (`tests/e2e-portal-interactions.js`)
  - deep services suite (`tests/services-deep-e2e.sh`)
  - contract write scenarios (`tests/contracts-write-e2e.py`)
  - contract deployment pipeline smoke
  - CLI comprehensive suite
- Deep service domains
  - faucet config/status/request/history coverage plus explorer-style readback, bridge deposit status lookup, and marketplace browse paths
  - custody withdrawal creation + wrapped-token burn + custody burn confirmation on the local stack
  - explorer → developers → programs portal handoff runtime coverage (cross-app URL rewrite, embedded IDE rewrite, copy/view-code controls)
  - token lifecycle write-path (`token create` + registry + mint)
  - launchpad contract discoverability + stats/event query checks
  - contract-by-contract inventory enforcement for all `contracts/*`
  - per-contract generic RPC checks (`getContractInfo`, `getProgramStats`, `getProgramStorage`, `getProgramCalls`, `getContractEvents`)
  - DEX REST API health (`/pairs`, `/tickers`, `/pools`)
  - DEX bootstrap pair assertion (`LICN/lUSD`, configurable)
  - faucet and custody live-service checks
  - DEX write scenarios in contract runner (`dex_core`, `dex_amm`, `dex_router`, `dex_margin`, `dex_rewards`, `dex_governance`, `dex_analytics`)
  - extended non-DEX write scenarios (`lichenid`, `bountyboard`, `lichenpunks`, `lichenoracle`, `lichenswap`, `lusd_token`, `weth_token`, `wsol_token`)
  - write-step enforcement includes transaction confirmation + program observability deltas (`getProgramCalls`/`getContractEvents`) for mutating actions
  - contract-level aggregate activity floors enforce minimum per-contract write activity deltas after each scenario block
  - explicit activity floor overrides for critical contracts (`dex_core`, `dex_router`, `dex_margin`, `lichenbridge`, `thalllend`)
  - domain-specific launch-blocking assertions: token lifecycle invariants, launch token flow, lending/bridge/storage/vault/bounty flows, LichenID identity lifecycle, prediction-market admin wiring, governance/job lifecycle, NFT ownership transitions, oracle freshness, swap reserve movement
  - adversarial/guardrail checks: unauthorized and duplicate actions must produce no unexpected state change
  - optional exact negative-code matching: reject-path steps can require specific error/return codes from transaction payload
  - scenario coverage floor: discovered deployed contracts must have corresponding write scenarios when enabled
  - adversarial depth floor: minimum count of negative assertions must execute to pass gate
  - expected-contract lockfile enforcement: discovered deployment compared against `tests/expected-contracts.json` with diff emitted in report artifact
- LichenID (read-path)
  - identity/reputation/skills/vouches/achievements
  - profile/directory/stats
  - resolve/reverse/batch name lookups

## Launch-blocking gaps to close for true A-to-Z

- LichenID write-path full lifecycle (transaction-path verified):
  - register identity
  - update profile/availability/rate/metadata
  - add skill + attest/revoke attest
  - vouch + cooldown semantics
  - name register/renew/transfer/release
  - social recovery set/approve/execute
  - delegation set/revoke + delegated write actions
  - premium-name auction create/bid/finalize
- Governance and launchpad lifecycle with real identity prerequisites:
  - bootstrap LichenID identity + reputation before proposing or voting
  - proposal create/vote/finalize/execute with post-execution state checks
  - launchpad creation/buy/sell/graduation into listed DEX pair
- DeFi and app contract state-changing flows under real funded actors:
  - launchpad creation/buy/graduation
  - DEX swap/add/remove liquidity
  - lending supply/borrow/repay/liquidation
  - bridge lock/mint/burn/redeem
  - marketplace list/buy/cancel
- Per-contract action completeness is still in progress for non-DEX domains and advanced edge paths (failure-path assertions, liquidation stress, proposal execution timing, multi-hop/router swap settlement).
- Custody production-path scenarios beyond local burn verification (real outbound Solana/EVM settlement confirmation, multi-signer payout completion, and failure-path handling)
- Wallet extension + fully rendered UI click-path automation tied to chain assertions
- Negative/adversarial state-changing cases per major contract

## Exit criteria

Release gate passes only when:

1. `tests/production-e2e-gate.sh` passes with `STRICT_NO_SKIPS=1`.
2. No launch-blocking gap remains unchecked in this matrix.
3. Any failed scenario blocks release until fixed and rerun clean.
