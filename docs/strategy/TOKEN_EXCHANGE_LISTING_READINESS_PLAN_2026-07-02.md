# Lichen Token Exchange Listing Readiness Plan

**Created:** 2026-07-02
**Status:** Planning and documentation only. No Lichen contract token, wrapped token, launchpad token, or DEX ecosystem token is exchange-ready from this plan alone.
**Tracker:** [TOKEN_EXCHANGE_LISTING_READINESS_TRACKER.md](./TOKEN_EXCHANGE_LISTING_READINESS_TRACKER.md)
**Related native package:** [EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md](./EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md)
**Scope:** Future exchange listings for fungible tokens issued, wrapped, launched, or traded on Lichen.
**Out of scope:** Native LICN deposits and withdrawals, NFTs, marketplace assets, shielded assets, and mainnet token listing claims before mainnet launch.
**Launchpad/DEX graduation dependency:** [LAUNCHPAD_DEX_GRADUATION_PLAN_2026-07-02.md](./LAUNCHPAD_DEX_GRADUATION_PLAN_2026-07-02.md)

## Executive Position

The native LICN exchange package is not a blanket approval for tokens on Lichen. Token listings add contract execution, token-specific metadata, integer scaling, registry integrity, holder indexes, transfer indexes, admin controls, reserve policy, custody flows, liquidity dependencies, and per-asset risk disclosure. That must be packaged and tested separately before any exchange is told that a token on Lichen is ready for listing.

The standard is the same as the native exchange plan: no guessing, no duplicated guidance, no paper readiness, no hidden dependencies, and no readiness claim until local and public testnet evidence is green. Every token listing must produce a per-token package with exact source-of-truth metadata, deposit and withdrawal procedures, RPC examples, archive/history guarantees, contract-risk status, and operational contacts.

## Current Source-Backed Facts

These facts are from repository inspection on 2026-07-02 and must be carried into implementation work:

| Area | Source-backed fact | Source files |
| --- | --- | --- |
| Token RPC | Canonical JSON-RPC dispatch exposes `getTokenAccounts`, `getTokenBalance`, `getTokenHolders`, `getTokenTransfers`, `getContractInfo`, `getContractEvents`, `getSymbolRegistry`, `getSymbolRegistryByProgram`, and `getAllSymbolRegistry`. | `rpc/src/lib.rs`, `docs/guides/RPC_API_REFERENCE.md`, `developers/rpc-reference.html` |
| Token balances | Token balances are indexed from contract storage keys containing `_bal_`; keys map token program plus holder to a raw `u64` balance. | `core/src/processor/contract_execution.rs`, `core/src/state/secondary_indexes.rs` |
| Holder token accounts | `getTokenAccounts` accepts `[holder_address]` and returns token program IDs, raw balances, registry-derived decimals, `ui_amount`, symbol, and name. | `rpc/src/lib.rs` |
| Token balance lookup | `getTokenBalance` accepts `[token_program, holder]`, returns raw `balance`, registry-derived decimals, `ui_amount`, and symbol. | `rpc/src/lib.rs` |
| Token holder scan | `getTokenHolders` accepts `[token_program, limit?, after_holder?]`, scans current token balance indexes, and caps limit at 1000. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` |
| Token transfer scan | `getTokenTransfers` accepts `[token_program, limit?, before_slot?]`, returns transfer rows with `from`, `to`, raw `amount`, slot, and optional tx hash. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` |
| Cold archive coverage | Token transfer indexes are included in cold public-history migration and `get_token_transfers` falls through to cold storage when attached. Balance and holder lookups are current-state indexes and need separate exchange validation. | `core/src/state/secondary_indexes.rs`, `core/src/state/cold_storage.rs` |
| Symbol registry | `SymbolRegistryEntry` stores `symbol`, `program`, `owner`, optional `name`, optional `template`, optional JSON `metadata`, and optional top-level `decimals`. Reverse lookup by program is stored in `CF_SYMBOL_BY_PROGRAM`. | `core/src/state.rs`, `core/src/state/program_state.rs` |
| Token decimals in RPC | RPC prefers top-level registry `decimals`, then metadata `decimals`, then defaults to 9. Raw integer balances remain the accounting source of truth. | `rpc/src/lib.rs` |
| Contract metadata | `getContractInfo` includes contract owner, code metadata, ABI summary, code hash, version, and token metadata for symbol-registered tokens. Capability flags such as mintable and burnable are ABI-derived. | `rpc/src/lib.rs`, `developers/rpc-reference.html` |
| Example standard token | `mt20_token` is a fungible token example with initialize, mint, burn, transfer, approve, allowance, transfer_from, and total_supply paths. | `contracts/mt20_token/src/lib.rs`, `contracts/mt20_token/abi.json` |
| Wrapped tokens | `lUSD`, `wSOL`, `wETH`, `wBNB`, `wNEO`, `wGAS`, and `wBTC` contracts expose mint, burn, transfer, approve, transfer_from, total_supply, emergency pause, admin/minter controls, and reserve or attestation semantics depending on asset. Current contract constants inspected on 2026-07-02 use 9 decimals. | `contracts/lusd_token/src/lib.rs`, `contracts/wsol_token/src/lib.rs`, `contracts/weth_token/src/lib.rs`, `contracts/wbnb_token/src/lib.rs`, `contracts/wneo_token/src/lib.rs`, `contracts/wgas_token/src/lib.rs`, `contracts/wbtc_token/src/lib.rs` |
| Wrapped-asset doc drift | `docs/defi/WRAPPED_ASSETS.md` contains older token-map rows that state lUSD as 6 decimals and wETH as 18 decimals, while the current contract constants inspected use 9. This is a release-blocking documentation/source-of-truth drift for any wrapped token exchange package. | `docs/defi/WRAPPED_ASSETS.md`, wrapped token contract sources |
| Launchpad surface | SporePump launchpad REST exposes `/api/v1/launchpad/tokens`, `/tokens/:id`, `/tokens/:id/quote`, and `/tokens/:id/holders`; graduated tokens are directed to trade on DEX. | `rpc/src/launchpad.rs` |
| DEX surface | DEX and router surfaces can provide liquidity, pairs, routes, and market context, but they are not a replacement for token custody, deposit, withdrawal, or exchange accounting verification. | `rpc/src/dex.rs`, `contracts/dex_*`, developer portal contract references |

## Token Listing Classes

Every future token package must declare exactly which class it is in. A token can belong to more than one class, but each class adds gates.

| Class | Description | Added gates |
| --- | --- | --- |
| Standard fungible contract token | A token contract deployed on Lichen with transfer and balance semantics similar to `mt20_token`. | Contract ABI freeze, symbol registry verification, transfer event/index verification, holder/balance archive policy, SDK/CLI transfer support, local token deposit/withdrawal simulation |
| First-party wrapped or reserve-backed token | lUSD, wSOL, wETH, wBNB, wNEO, wGAS, wBTC, or similar reserve-backed receipt asset. | Reserve policy, custody runbook, mint/burn authority review, attestation/proof policy, emergency pause policy, redemption runbook, source-chain finality rules |
| Launchpad token | Token created through SporePump or another Lichen launchpad path. | Launchpad source data, graduation status, creator controls, bonding-curve state, DEX migration state, holder distribution, liquidity threshold, anti-spam and abuse review |
| DEX-listed ecosystem token | Token with a Lichen DEX pair, route, or liquidity campaign. | Pair/pool ID verification, quote asset policy, liquidity depth, market integrity, pause/restriction state, oracle dependency if used |
| Compatibility view token | Token exposed through EVM or Solana compatibility views. | Compatibility endpoint parity, address mapping, event/log mapping, duplicate-credit prevention between canonical and compatibility paths |

## Readiness Standard

Token exchange readiness means an exchange engineer can perform the full lifecycle for a specific token without private help:

1. Identify the exact token program or contract ID.
2. Validate symbol, name, decimals, logo URL, issuer, owner/admin, and chain/network scope.
3. Generate or assign a Lichen deposit address for that token.
4. Detect token deposits through a documented, archive-backed method.
5. Credit user balances exactly once using raw integer base units.
6. Build, sign, broadcast, and monitor token withdrawals.
7. Retry polling and broadcasts without duplicate credits or duplicate withdrawals.
8. Reconcile hot wallet, cold wallet, exchange ledger, token balances, holder indexes, transfer history, fees, and native LICN fee spend.
9. Query old token transfers and related transactions indefinitely from an archive-backed endpoint.
10. Understand contract admin powers, pause state, mint/burn policy, reserve policy, and incident contacts before listing.

## Non-Negotiables

- Native LICN exchange readiness does not imply token exchange readiness.
- No token may be described as exchange-ready until its own tracker row is green.
- No exchange guide may publish guessed token decimals, symbols, contract IDs, logos, issuer names, reserve ratios, or admin keys.
- Exchange accounting must use raw integer base units. `ui_amount`, floating-point display values, and JavaScript number parsing are display-only unless lossless integer parsing is proven.
- Token metadata must come from a signed package plus registry/source verification. Registry data alone is not enough for external listing.
- Token deposit detection must survive RPC retry, validator restart, hot/cold migration, and archive reopen.
- Token withdrawal broadcast must have idempotency keys and a duplicate-send prevention story.
- Wrapped and reserve-backed tokens require reserve, custody, burn/redemption, proof, pause, and incident policies before any listing claim.
- Launchpad tokens require separate maturity and market-integrity gates before exchange outreach.
- Any mainnet token package is deferred until mainnet launch and must rerun the full token readiness gate against mainnet, not only testnet.

## Required Deliverables

### Token Exchange Integration Guide

Target files:

- `docs/guides/TOKEN_EXCHANGE_INTEGRATION.md`
- `developers/token-exchange-integration.html`

Required content:

- Explicit testnet-only scope until mainnet launch.
- Token classes and per-token package rule.
- Token address model: native Lichen account addresses hold token balances; token program/contract ID identifies the asset.
- Token metadata: symbol, name, decimals, contract/program ID, registry entry, owner/admin, issuer, logo URL, explorer URLs, ABI hash, code hash, and release tag.
- Deposit cookbook: deposit address assignment, polling with `getTokenTransfers`, cross-checks with `getTransaction` and `getTokenBalance`, retry rules, idempotency keys, and credit policy.
- Withdrawal cookbook: token transfer construction, native LICN fee funding, hot/cold model, broadcast, polling, retries, stuck withdrawal policy, and reconciliation.
- Archive and history policy: which token methods must be archive-backed, which are current-state only, and how exchanges should combine transaction, transfer, balance, and holder queries.
- Wrapped token section: reserve and redemption model, custody status, pause policy, mint/burn controls, and reserve proof requirements.
- Launchpad/DEX token section: maturity, graduation status, liquidity, pair IDs, market integrity, and abuse-review requirements.
- Compatibility section: EVM/Solana compatibility views are optional and must never replace canonical JSON-RPC until separately validated.

### Token Metadata Sheet

Target files:

- `docs/guides/TOKEN_EXCHANGE_METADATA.md`
- `docs/guides/TOKEN_EXCHANGE_VALIDATION_VECTORS.md`

Each token must have:

| Field | Required source |
| --- | --- |
| Chain/network scope | Native network docs plus testnet/mainnet package label |
| Token class | This plan's class table |
| Contract/program ID | Deployed source, release artifact, registry reverse lookup |
| Symbol and name | Symbol registry plus signed per-token manifest |
| Decimals | Contract source or deployed registry top-level decimals plus test vector |
| Base unit name | Per-token docs; default raw `u64` units when no named unit exists |
| Owner/admin/minter/pauser | Contract state, governance record, or deployment manifest |
| ABI and code hash | `getContractInfo`, release artifacts, source build evidence |
| Total supply method | Contract getter and RPC example |
| Balance method | `getTokenBalance` and `getTokenAccounts` examples |
| Transfer-history method | `getTokenTransfers` plus `getTransaction` examples |
| Logo URL | Public immutable asset path with dimensions and hash |
| Explorer URLs | Address, transaction, block, and token/program routes |
| Status and contacts | Exchange operations pack |
| Reserve proof | Required for wrapped/reserve-backed tokens only |
| DEX pair IDs | Required for DEX-listed tokens only |
| Launchpad token ID | Required for launchpad tokens only |

### Token Operations Pack

Target files:

- `docs/deployment/TOKEN_EXCHANGE_OPERATIONS_PACK.md`
- Link from developer portal token exchange page

Required content:

- Token incident contacts and escalation windows.
- Token pause/emergency policy.
- Contract upgrade policy and immutability expectations.
- Token metadata correction policy.
- Wrapped-asset reserve incident policy.
- Launchpad abuse and fraud-response policy.
- Release-signature verification for token builds and token listing manifests.
- Rollback and delisting criteria for token-specific incidents.
- Mainnet handoff requirements for future token listings.

### Token E2E Simulation

Target implementation after this documentation phase:

- `scripts/qa/token_exchange_simulation.py`
- Unit and integration tests under the relevant Rust/Python/JS test locations.
- Evidence under ignored local paths such as `evidence/token-exchange-readiness/<token>/<date>/`.

Required flow for a standard fungible token:

1. Start a clean local validator stack with four validators.
2. Prove each validator can stop and rejoin before public testnet deployment.
3. Deploy or select a test token contract with known symbol, decimals, ABI, and code hash.
4. Register the token symbol and metadata.
5. Generate exchange hot, cold, deposit, and withdrawal destination accounts.
6. Fund native LICN for fees.
7. Mint or transfer test token units to a customer source wallet.
8. Send token units into the exchange deposit address.
9. Detect the deposit through transfer history and transaction lookup.
10. Cross-check deposit address token balance and account token list.
11. Wait for deterministic finality plus the operational buffer selected for token listings.
12. Credit an internal ledger once using raw integer units.
13. Sweep if the token custody model requires sweeping.
14. Withdraw token units to the customer destination.
15. Poll withdrawal transaction and token transfer history.
16. Restart one validator and prove token history and balances remain queryable.
17. Trigger hot/cold archive migration and reopen, then prove token transfer history and related native transactions survive.
18. Reconcile token balances, native LICN fees, exchange ledger, holder index, transfer index, and transaction IDs.
19. Stop the stack and verify cleanup.

Required extra flow for wrapped tokens:

1. Simulate external deposit detection through custody mocks.
2. Mint the wrapped token to the customer on Lichen.
3. Verify reserve/liability accounting before and after mint.
4. Burn wrapped token for withdrawal.
5. Verify custody release status and reserve/liability reduction.
6. Prove emergency pause behavior does not trap legitimate redemption paths beyond the documented policy.

Required extra flow for launchpad and DEX tokens:

1. Read launchpad token state through launchpad REST.
2. Verify token ID, creator, supply, graduation status, and holder balance.
3. If graduated, verify DEX pair/pool IDs and liquidity.
4. Prove the exchange package does not rely on quote endpoints for custody accounting.
5. Verify listing maturity rules and abuse-review criteria.

## Workstreams And Gates

### Phase 0: Source Map And Drift Inventory

Status: started by this plan and tracker.

Purpose: establish source-backed facts before writing external token guides or code.

Tasks:

- Map token RPC methods and exact params/results.
- Map token balance, holder, transfer, event, and symbol-registry storage.
- Map token transfer archive/cold-store behavior.
- Map standard token contract semantics.
- Map wrapped token contracts, reserve semantics, mint/burn controls, and pause behavior.
- Map launchpad REST and DEX market surfaces.
- Record every doc/code drift item, especially token decimals and source-of-truth conflicts.

Exit gate:

- Tracker has source map rows with files, status, and blockers.
- Every token readiness claim is either backed by source evidence or marked blocked.

### Phase 1: Token Listing Policy And Metadata Model

Status: not started.

Purpose: freeze the rules for what a token listing package must contain.

Tasks:

- Define per-token listing manifest schema.
- Define token class labels and class-specific required fields.
- Define source-of-truth priority: deployed state, signed manifest, registry, source code, docs.
- Define token decimal and raw-unit validation vectors.
- Define registry update and metadata correction policy.

Exit gate:

- Token manifest schema is documented.
- Registry and manifest conflicts fail the readiness gate.
- Wrapped-token decimal drift is resolved before any wrapped token listing claim.

### Phase 2: Token Exchange Guide And Developer Portal

Status: not started.

Purpose: create the external exchange-facing docs for token listings.

Tasks:

- Create `docs/guides/TOKEN_EXCHANGE_INTEGRATION.md`.
- Create `developers/token-exchange-integration.html`.
- Link the page from the developer portal without replacing the native LICN guide.
- Add a visible warning that token readiness is per-token and testnet-only until mainnet launch.
- Include exact RPC examples only after they are verified.

Exit gate:

- GitHub docs and developer portal carry the same substance.
- The native LICN guide links to token listing docs only as a separate package.
- The token developer page is tested by a frontend asset/readiness check.

### Phase 3: Token RPC, Index, And Archive Verification

Status: not started.

Purpose: prove that token deposit detection and historical lookup survive production-like history handling.

Tasks:

- Add focused tests for `getTokenTransfers` hot and cold behavior.
- Add focused tests for `getTokenBalance`, `getTokenAccounts`, and `getTokenHolders` after restart.
- Verify event and transaction lookup correlation for token transfers.
- Verify pagination and `before_slot` behavior.
- Verify null, unknown token, unknown holder, malformed address, and limit handling.
- Decide whether exchanges should use token transfer index, contract events, transaction parsing, or a combination.

Exit gate:

- Token transfer history passes hot/cold migration and reopen tests.
- Balance and holder behavior is documented as current-state or archival with proof.
- Public testnet token transfer history is verified against old and new transfers.

### Phase 4: Token Transaction Construction And Broadcast

Status: not started.

Purpose: document and test how exchanges create token withdrawals.

Tasks:

- Identify the canonical token transfer instruction format.
- Add or verify CLI token transfer support.
- Add or verify SDK token transfer builders with lossless integer handling.
- Document native LICN fee-funding requirements for token withdrawals.
- Define idempotency keys and retry behavior for token withdrawal jobs.

Exit gate:

- Token withdrawal cookbook is executable from CLI/SDK examples.
- Duplicate broadcast protection is tested.
- Exact integer accounting is tested at safe and large `u64` values.

### Phase 5: Contract Risk And Admin Controls

Status: not started.

Purpose: prevent exchanges from listing tokens without understanding mutability and emergency powers.

Tasks:

- Document owner, admin, minter, pauser, attester, and governance roles per token class.
- Verify `getContractInfo` code hash, ABI, and version for listed tokens.
- Verify total supply, mint, burn, pause, transfer, approve, and transfer_from semantics.
- Define token upgrade, freeze, pause, and emergency communication policy.
- Define listing rejection criteria for unsafe or unaudited contracts.

Exit gate:

- Every token listing has a contract risk sheet.
- Admin powers are visible in the exchange package.
- Pause and emergency state are queryable or operationally published.

### Phase 6: Wrapped Asset, Reserve, And Custody Package

Status: not started.

Purpose: make reserve-backed token listings credible.

Tasks:

- Reconcile wrapped-token docs with current contract constants and deployed registry metadata.
- Document reserve custody model, supported source chains, source-chain finality, sweep, mint, burn, and redemption.
- Document proof-of-reserve and liability reconciliation.
- Verify reserve attestation paths and custody service health.
- Define reserve incident, withdrawal delay, pause, and delisting policy.

Exit gate:

- Reserve/liability simulation passes locally.
- Public testnet wrapped-token mint/burn/redemption simulation passes where test infrastructure supports it.
- Exchange package includes reserve proof and custody operations docs.

### Phase 7: Launchpad And DEX Token Policy

Status: not started.

Purpose: avoid presenting speculative or immature tokens as exchange-ready.

Tasks:

- Define minimum maturity rules for launchpad tokens.
- Define graduated-token requirements before external exchange outreach.
- Define DEX liquidity, holder distribution, and market-integrity thresholds.
- Verify launchpad REST and DEX pair/pool data against on-chain state.
- Document that DEX quotes are trading context, not deposit accounting.

Exit gate:

- Launchpad token package includes token ID, creator, supply, graduation status, holder distribution, and risk flags.
- DEX token package includes pair IDs, liquidity, route status, pause/restriction status, and market-integrity review.

### Phase 8: Local Token Exchange Simulation

Status: not started.

Purpose: prove the full token lifecycle before touching public testnet.

Tasks:

- Build local token exchange simulation.
- Run on a clean four-validator stack.
- Run restart/rejoin drill for each local validator before public testnet deployment.
- Include token deposit, credit, withdrawal, archive, restart, and cleanup.
- Include wrapped and launchpad variants when those token classes are in scope.

Exit gate:

- Local simulation report is green.
- Validator restart/rejoin is green.
- Cleanup is verified.
- No public testnet deployment or token listing claim happens before this gate.

### Phase 9: Public Testnet Token Package

Status: not started.

Purpose: prove the token package on public testnet.

Tasks:

- Deploy or select token artifacts through the deployment runbook.
- Verify public RPC, WebSocket, explorer, developer portal, and status page.
- Run public testnet token exchange simulation.
- Capture evidence for token transfers, balances, holder scans, archive history, and explorer pages.
- Confirm public testnet remains healthy after the run.

Exit gate:

- Public testnet token simulation is green.
- Public readiness gate for token listings passes with status approval and release tag selection.
- Package remains explicitly testnet-only until mainnet launch.

### Phase 10: External Token Listing Package

Status: not started.

Purpose: package a token for exchange review.

Tasks:

- Publish signed per-token metadata manifest.
- Publish token integration guide and developer portal page.
- Publish token operations pack.
- Publish release tag or package tag.
- Publish validation report and evidence paths.
- Record exact rollback and delisting policy.

Exit gate:

- Token package tag is selected.
- CI and readiness gates are green.
- Operator approval is recorded.
- Mainnet scope remains deferred unless full mainnet token readiness has passed.

## Mainnet Handoff

All token listing work remains testnet-only until mainnet launch. Mainnet token readiness must not inherit any testnet package result. At mainnet launch, each token must rerun:

- Source and deployed-state verification.
- Token metadata manifest verification.
- Token RPC/index/archive tests.
- Local validator simulation on the release candidate.
- Public mainnet RPC/WebSocket/explorer/status verification.
- Mainnet token deposit and withdrawal simulation where safe and applicable.
- Reserve/custody proof verification for wrapped tokens.
- Final external package publication and signed release/tag selection.

## Current Completion Statement

This document and the tracker start the token exchange listing work. They do not make any token ready for exchange listing. The next ordered task is Phase 0 completion: reconcile all source/doc drift, then write the external token guide and token metadata schema before implementing or claiming any token readiness.
