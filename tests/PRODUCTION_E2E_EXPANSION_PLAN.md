# Production E2E Expansion Plan

This plan turns the existing launch-blocking matrix into an execution tracker for full user-journey coverage. The target is simple: every user-facing action that Lichen exposes must have at least one real transaction-path E2E and at least one failure-path assertion before redeploy.

Use this document with `tests/PRODUCTION_E2E_MATRIX.md`, not instead of it.

## Recent major changes that must stay under gate

- Canonical keypairs now use the shared encrypted-at-rest format across `core`, `validator`, `genesis`, `custody`, `cli`, and the Python SDK. Production paths require `LICHEN_KEYPAIR_PASSWORD` outside explicit local-dev mode.
- The canonical local 3-validator launcher now rebuilds stale release binaries and refreshes changed contract WASM before startup. Local validation must keep exercising `scripts/start-local-3validators.sh`, not ad hoc validator starts.
- The full-matrix harness now reuses a healthy full local stack only when validators and faucet are already healthy. Matrix runs must not silently tear a good full stack down into validators-only mode.
- Wrapped-token authority is split: governance owns long-lived admin control, custody keeps the operational minter key until governance rotates it, and attester rotation goes through the oracle-committee approval lane.
- `getTransaction` now returns `null` until a transaction is indexed in a block. E2E helpers must confirm first and only treat indexed transaction payloads as optional enrichment.
- DEX governance is reputation-gated through LichenID. Proposal and vote suites must bootstrap identity and reputation instead of skipping governance paths.

## Non-negotiable rules

- No skip is acceptable for a missing prerequisite that the test harness can create itself.
- Every major feature needs one positive path and one explicit negative path.
- UI automation only counts when it also verifies chain state or service state after the click path.
- Full-stack suites must run against validators, custody, and faucet whenever the feature depends on those services.
- All new suites must be safe to rerun on a reused healthy local stack.

## Strict execution order

1. Harness foundations
2. Identity, reputation, and naming bootstrap
3. Governance and launchpad lifecycle
4. Spot DEX, router, liquidity, and rewards
5. Lending, margin, stop-loss, and liquidation
6. Prediction market lifecycle and settlement
7. NFT, marketplace, and auction flows
8. Bridge, wrapped tokens, custody, and oracle-dependent settlement
9. Developer lifecycle and UI click-path automation

This order is intentional. Governance depends on identity and reputation. Launchpad graduation depends on DEX listing and liquidity. Liquidation depends on oracle, lending, and margin state. Custody and bridge settlement depend on wrapped-token issuance and burn correctness.

## Shared fixtures to build first

- A common actor factory that creates and funds named roles: `developer`, `governor_a`, `governor_b`, `governor_c`, `governor_d`, `lp_provider`, `trader_a`, `trader_b`, `borrower`, `liquidator`, `predictor_a`, `predictor_b`, `nft_creator`, `buyer`, `bridge_user`.
- Reusable funding helpers that prefer local RPC airdrops, fall back to funded signers only when necessary, and always wait on confirmation before asserting balance.
- Identity bootstrap helpers that can register LichenID, attach reputation fixtures, register names, and verify post-write reads.
- Oracle freshness helpers that can block until price feeds are live before testing liquidation, settlement, or bridge logic.
- Full-stack readiness helpers that assert validator quorum, faucet health, custody health, and deployer key availability before a suite starts.

## Suite map

### Phase 1: Harness foundations

Extend:

- `tests/production-e2e-gate.sh`
- `tests/run-full-matrix-feb24.sh`
- `tests/e2e-developer-lifecycle.py`
- `tests/e2e-custody-withdrawal.py`

Required outcomes:

- one transaction-wait policy everywhere: confirm first, fetch indexed payload second
- one funding policy everywhere: local airdrop helpers first, donor fallback second
- one cluster reuse policy everywhere: never reset a healthy full stack unless explicitly requested
- explicit preflight failure when custody or faucet is required but missing

Acceptance criteria:

- no remaining `Transaction not found` polling loops as the primary success signal
- no suite silently downgrades from full stack to validators-only mode
- matrix preflight fails fast when required sidecars are absent

### Phase 2: Identity, reputation, and naming bootstrap

Add or expand:

- `tests/e2e-lichenid-lifecycle.py`
- `tests/contracts-write-e2e.py`

User actions to cover:

- register identity
- update profile, availability, rate, and metadata
- add skill
- attest skill and revoke attestation
- vouch and enforce cooldown semantics
- register, renew, transfer, and release a name
- configure social recovery and execute recovery flow
- configure delegation, perform delegated write, revoke delegation
- run premium-name auction create, bid, and finalize

Negative paths:

- duplicate registration
- unauthorized delegated write
- cooldown bypass
- expired or malformed recovery flow

Acceptance criteria:

- governance suites consume this bootstrap instead of skipping on missing reputation
- every write action has a post-read assertion through RPC or contract storage inspection

### Phase 3: Governance and launchpad lifecycle

Add or expand:

- `tests/e2e-launchpad.js`
- `tests/e2e-governance-lifecycle.js`
- `tests/contracts-write-e2e.py`

User actions to cover:

- create proposal with reputation-backed identity
- cast yes and no votes from distinct actors
- prevent double-vote
- reject low-reputation proposer and voter
- finalize only after voting window ends
- execute only after pass and quorum
- verify downstream state change on target contract after execution
- create launchpad token
- buy and sell during launch phase
- graduate into listed DEX pair
- verify post-graduation pair visibility and liquidity bootstrap

Specific regression target:

- zero-vote governance cards must display zero votes, not synthetic `50/50`

Acceptance criteria:

- no governance path is skipped for missing LichenID setup
- at least one proposal executes and changes live state during gate runs
- launchpad graduation is verified by downstream DEX visibility, not only by launchpad-local reads

### Phase 4: Spot DEX, router, liquidity, and rewards

Add or expand:

- `tests/e2e-dex-liquidity-router.py`
- `tests/contracts-write-e2e.py`
- `tests/e2e-portal-interactions.js` where UI interaction is required

User actions to cover:

- create order
- cancel order
- partial fill and full fill
- direct swap
- router quote and router swap
- add liquidity
- remove liquidity
- claim rewards
- verify fee accrual and treasury movement

Negative paths:

- insufficient liquidity
- stale route
- invalid pair
- remove more liquidity than owned

Acceptance criteria:

- both orderbook and AMM paths are exercised by funded actors
- router path is verified against resulting balances, not only quote response

### Phase 5: Lending, margin, stop-loss, and liquidation

Add or expand:

- `tests/e2e-lending-margin.py`
- `tests/e2e-margin-liquidation.py`
- `tests/contracts-write-e2e.py`

User actions to cover:

- supply collateral
- borrow
- repay
- withdraw collateral
- open long and short positions
- reduce and close positions
- configure stop-loss or equivalent protective exit if supported on-chain
- trigger liquidation through oracle movement or controlled price setup
- verify insurance fund or liquidation accounting deltas

Negative paths:

- over-borrow
- under-collateralized open
- invalid liquidation before threshold
- close more than position size

Acceptance criteria:

- at least one real liquidation occurs during the suite
- liquidation verifies borrower, liquidator, and protocol accounting, not only return codes

### Phase 6: Prediction market lifecycle and settlement

Add or expand:

- `tests/e2e-prediction-market.py`
- `tests/contracts-write-e2e.py`

User actions to cover:

- create market
- take yes and no positions
- close market
- resolve market
- settle winning position
- reject losing or duplicate settlement

Acceptance criteria:

- resolution source and final payout deltas are asserted
- unresolved markets cannot be settled successfully

### Phase 7: NFT, marketplace, and auction flows

Add or expand:

- `tests/e2e-marketplace-auction.py`
- `tests/contracts-write-e2e.py`

User actions to cover:

- mint NFT
- list for sale
- buy
- cancel listing
- create auction
- bid from multiple actors
- finalize auction
- verify ownership transfer and funds transfer

Negative paths:

- buy cancelled listing
- finalize before end
- bid below minimum

Acceptance criteria:

- ownership changes are validated through both contract reads and user-facing browse endpoints

### Phase 8: Bridge, wrapped tokens, custody, and oracle-dependent settlement

Add or expand:

- `tests/e2e-custody-withdrawal.py`
- `tests/e2e-bridge-custody.py`
- `tests/contracts-write-e2e.py`

User actions to cover:

- bridge deposit status lookup
- wrapped-token mint by operational minter
- wrapped-token burn for withdrawal
- custody withdrawal creation
- custody confirmation and payout completion
- attester update and minter rotation governance lanes
- reserve attestation freshness checks

Negative paths:

- mint by non-minter
- stale reserve attestation
- payout confirmation with mismatched burn
- duplicate withdrawal confirmation

Acceptance criteria:

- custody suite proves the full burn-to-confirmation path on the local full stack
- governance-controlled wrapped-token admin and operational minter split remains covered by regression assertions

### Phase 9: Developer lifecycle and UI click-path automation

Add or expand:

- `tests/e2e-developer-lifecycle.py`
- new UI automation under `tests/ui/` or an equivalent dedicated folder

User actions to cover:

- developer wallet creation and funding
- contract deploy
- contract read
- contract write
- contract upgrade if supported on the target path
- portal to programs handoff
- wallet connect flows in DEX, marketplace, and developer portal
- governance vote button path
- marketplace purchase path

Acceptance criteria:

- UI tests assert resulting on-chain state, not just rendered DOM
- developer lifecycle goes beyond deploy/read/call and covers post-deploy maintenance actions

## File ownership guidance

- `tests/e2e-developer-lifecycle.py`: developer and contract lifecycle, shared balance and confirmation helpers
- `tests/e2e-launchpad.js`: governance and launchpad end-to-end UI-adjacent flow
- `tests/contracts-write-e2e.py`: broad contract action completeness, negative assertions, and discovered-contract enforcement
- `tests/production-e2e-gate.sh`: launch-blocking order and strictness
- `tests/run-full-matrix-feb24.sh`: cluster reuse, sidecar preservation, and matrix orchestration

## Release gate exit criteria

- `STRICT_NO_SKIPS=1` passes cleanly
- every user-facing domain above has at least one dedicated lifecycle suite and contract-write coverage where applicable
- no governance or launchpad step is skipped due to missing identity or reputation bootstrap
- no feature that depends on custody or faucet runs against a degraded validators-only stack
- matrix report shows zero missing scenario contracts for deployed programs
- the matrix document and this plan have no unchecked launch-blocking gap left open