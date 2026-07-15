# Lichen Token Exchange Listing Readiness Tracker

**Created:** 2026-07-02
**Plan:** [TOKEN_EXCHANGE_LISTING_READINESS_PLAN_2026-07-02.md](./TOKEN_EXCHANGE_LISTING_READINESS_PLAN_2026-07-02.md)
**Current phase:** Phase 0 source map and drift inventory.
**Current status:** Not exchange-ready for future token listings. Native LICN readiness remains separate and does not cover contract tokens, wrapped tokens, launchpad tokens, or DEX ecosystem tokens.
**Rule:** A future token can be called exchange-ready only when its own per-token gates are green and its package is explicitly published for the intended scope.

## Gate Status

| ID | Gate | Status | Release blocker | Evidence / source |
| --- | --- | --- | --- | --- |
| T0-01 | Dedicated token exchange plan created | Done | No | `docs/strategy/TOKEN_EXCHANGE_LISTING_READINESS_PLAN_2026-07-02.md` |
| T0-02 | Dedicated token exchange tracker created | Done | No | This tracker |
| T0-03 | Token source map started | In progress | Yes | Source map below |
| T0-04 | Token source/doc drift inventory completed | Open | Yes | Initial drift rows below; must be completed before external token docs |
| T1-01 | Per-token listing manifest schema documented | Open | Yes | Planned `docs/guides/TOKEN_EXCHANGE_METADATA.md` |
| T1-02 | Token class policy documented | Open | Yes | Plan class table exists; external guide still required |
| T1-03 | Token decimals/source-of-truth policy documented | Open | Yes | Wrapped-token decimals drift blocks readiness |
| T2-01 | Token exchange integration guide created | Open | Yes | Planned `docs/guides/TOKEN_EXCHANGE_INTEGRATION.md` |
| T2-02 | Developer portal token exchange page created | Open | Yes | Planned `developers/token-exchange-integration.html` |
| T2-03 | Native LICN exchange guide cross-reference added | Open | No | Must preserve native/token package separation |
| T3-01 | Token RPC parameter docs verified against source | Open | Yes | Existing developer portal docs appear partially inconsistent with `rpc/src/lib.rs` param order for `getTokenBalance` |
| T3-02 | Token transfer archive tests added and green | Open | Yes | `get_token_transfers` has cold fallback; test gate still required |
| T3-03 | Token balance/holder restart behavior tested | Open | Yes | Current-state index behavior must be proven and documented |
| T3-04 | Token transfer to transaction correlation tested | Open | Yes | Required before deposit detection guide |
| T4-01 | Token withdrawal construction documented and tested | Open | Yes | CLI/SDK support must be verified or added later |
| T4-02 | Token withdrawal duplicate-broadcast prevention tested | Open | Yes | Required before exchange cookbook |
| T4-03 | Native LICN fee funding for token withdrawals documented | Open | Yes | Token withdrawals consume native network fees |
| T5-01 | Contract risk sheet template created | Open | Yes | Planned token operations/risk docs |
| T5-02 | Contract owner/admin/minter/pauser policy documented | Open | Yes | Required for wrapped and mutable tokens |
| T5-03 | ABI/code-hash/release verification documented | Open | Yes | Required for each token |
| T6-01 | Wrapped asset reserve and custody package documented | Open | Yes | Planned `docs/deployment/TOKEN_EXCHANGE_OPERATIONS_PACK.md` |
| T6-02 | Wrapped token decimal drift resolved | Open | Yes | `docs/defi/WRAPPED_ASSETS.md` differs from inspected contract constants |
| T6-03 | Reserve/liability simulation designed | Open | Yes | Required for wrapped token listing claims |
| T7-01 | Launchpad token maturity policy documented | Open | Yes | Launchpad tokens need separate maturity and abuse review |
| T7-02 | DEX liquidity and market-integrity policy documented | Open | Yes | DEX presence alone is not exchange readiness; see `docs/strategy/LAUNCHPAD_DEX_GRADUATION_PLAN_2026-07-02.md` |
| T8-01 | Local token exchange simulation implemented | Open | Yes | Planned `scripts/qa/token_exchange_simulation.py` |
| T8-02 | Local four-validator token simulation green | Open | Yes | Must pass before public testnet token validation |
| T8-03 | Local restart/rejoin token simulation green | Open | Yes | Required deployment discipline |
| T8-04 | Local cleanup verified | Open | Yes | Required before public testnet |
| T9-01 | Public testnet token simulation green | Open | Yes | No token public readiness until this passes |
| T9-02 | Public developer portal token page verified | Open | Yes | Must be public and inline, not only a GitHub redirect |
| T9-03 | Token public readiness gate implemented and green | Open | Yes | Planned `scripts/qa/token_exchange_public_readiness.py` |
| T10-01 | External token listing package published | Open | Yes | Per-token package tag required |
| T10-02 | Mainnet token handoff documented and deferred | In progress | Yes | Plan contains mainnet handoff; external docs still needed |

## Source Map

| Area | Current finding | Source files | Status |
| --- | --- | --- | --- |
| Token RPC routing | Canonical JSON-RPC dispatch includes token, contract, event, and symbol registry methods. | `rpc/src/lib.rs` | Source mapped |
| `getTokenAccounts` | Source accepts `[holder_address]`; returns token program IDs as `mint`, raw balance, decimals, `ui_amount`, symbol, and name. | `rpc/src/lib.rs` | Source mapped |
| `getTokenBalance` | Source accepts `[token_program, holder]`; existing developer portal table names owner before token mint, which must be reconciled before external token docs. | `rpc/src/lib.rs`, `developers/rpc-reference.html` | Drift found |
| `getTokenHolders` | Source accepts `[token_program, limit?, after_holder?]`; limit max is 1000. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` | Source mapped |
| `getTokenTransfers` | Source accepts `[token_program, limit?, before_slot?]`; returns transfer rows with from, to, amount, slot, and tx hash. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` | Source mapped |
| Token balance indexing | Contract storage changes with `_bal_` keys update token balance indexes. | `core/src/processor/contract_execution.rs` | Source mapped |
| Token balance storage | `CF_TOKEN_BALANCES` stores token program plus holder to raw `u64` balance; balance and holder scan currently read hot/current state. | `core/src/state.rs`, `core/src/state/secondary_indexes.rs` | Needs exchange tests |
| Token transfer storage | `CF_TOKEN_TRANSFERS` stores token program plus slot plus sequence to serialized `TokenTransfer`. | `core/src/state.rs`, `core/src/state/secondary_indexes.rs` | Source mapped |
| Token transfer archive | Cold public-history migration includes `CF_TOKEN_TRANSFERS`; `get_token_transfers` reads cold and hot rows with dedupe. | `core/src/state/cold_storage.rs`, `core/src/state/secondary_indexes.rs` | Needs focused tests |
| Symbol registry | Symbol registry stores symbol, program, owner, optional name, template, metadata, and decimals; duplicate symbols are rejected. | `core/src/state.rs`, `core/src/state/program_state.rs` | Source mapped |
| Token decimals | RPC prefers top-level registry decimals, then metadata decimals, then defaults to 9. | `rpc/src/lib.rs` | Source mapped |
| Contract metadata | `getContractInfo` exposes contract metadata and token metadata for registry-backed tokens. | `rpc/src/lib.rs`, `developers/rpc-reference.html` | Needs per-token vectors |
| Standard token example | `mt20_token` has initialize, mint, burn, transfer, approve, allowance, transfer_from, and total_supply. | `contracts/mt20_token/src/lib.rs` | Source mapped |
| Wrapped token contracts | Inspected wrapped token contract constants use 9 decimals for lUSD, wSOL, wETH, wBNB, wNEO, wGAS, and wBTC. | `contracts/lusd_token/src/lib.rs`, `contracts/w*_token/src/lib.rs` | Source mapped |
| Wrapped asset docs | Top token map in wrapped-assets docs still shows lUSD as 6 and wETH as 18 while later rows and contract constants show 9. | `docs/defi/WRAPPED_ASSETS.md`, wrapped token contracts | Release blocker |
| Launchpad REST | Launchpad REST exposes stats, config, token list, token detail, quote, and holder-balance endpoints. Current source does not yet implement real DEX graduation; see the dedicated graduation plan. | `rpc/src/launchpad.rs`, `docs/strategy/LAUNCHPAD_DEX_GRADUATION_PLAN_2026-07-02.md` | Source mapped |
| DEX surface | DEX routes and pair data exist, but they are trading context and not a substitute for token custody/accounting readiness. | `rpc/src/dex.rs`, `contracts/dex_*` | Needs listing policy |

## Initial Blockers

| ID | Blocker | Impact | Required next step |
| --- | --- | --- | --- |
| TB-01 | No token exchange integration guide exists | Exchanges cannot integrate future tokens from current docs | Create `docs/guides/TOKEN_EXCHANGE_INTEGRATION.md` after Phase 0 is complete |
| TB-02 | No per-token metadata manifest schema exists | Token symbol, decimals, code hash, owner, logo, and registry claims can drift | Define and document the schema |
| TB-03 | Token RPC docs appear inconsistent with source for `getTokenBalance` parameter order | Exchange examples could call the method incorrectly | Reconcile source, docs, tests, and developer portal before publication |
| TB-04 | Wrapped token decimal source-of-truth drift exists | Exchanges could credit the wrong amount for lUSD or wETH | Reconcile contract constants, registry metadata, docs, and validation vectors |
| TB-05 | Token transfer archive behavior is source-mapped but not exchange-tested | Deposit history cannot be claimed archive-safe yet | Add hot/cold/reopen token transfer tests |
| TB-06 | Balance and holder indexes are current-state indexes and not yet documented for exchange use | Exchanges might use holder scans incorrectly for historical deposits | Define detection and reconciliation strategy |
| TB-07 | Token transaction construction is not packaged for exchanges | Withdrawals cannot be safely implemented from docs alone | Document and test token transfer builders in CLI/SDKs |
| TB-08 | Contract admin and pause powers are not packaged per token | Exchanges lack listing risk controls | Create contract risk sheet template and per-token gate |
| TB-09 | Wrapped-asset reserve and redemption package is not exchange-ready | Reserve-backed token listings would be unsupported | Create reserve/custody operations pack and simulation |
| TB-10 | Launchpad and DEX token maturity policy is not implemented | Speculative tokens could be presented as listing-ready too early | Execute the dedicated launchpad-to-DEX graduation plan, then define maturity, liquidity, holder, and abuse-review gates |
| TB-11 | No token exchange simulation exists | No proof of deposit, credit, withdrawal, archive, restart, and cleanup | Implement after docs are approved |
| TB-12 | No public token readiness gate exists | Final publication cannot fail closed | Implement `token_exchange_public_readiness.py` later |
| TB-13 | Mainnet token package is not in scope yet | Mainnet claims would be premature | Keep all token package docs testnet-only until mainnet launch handoff |

## Phase Tracker

### Phase 0: Source Map And Drift Inventory

Status: in progress.

| Task | Status | Evidence |
| --- | --- | --- |
| Map token RPC methods | In progress | Source map rows for `rpc/src/lib.rs` |
| Map token storage/index behavior | In progress | Source map rows for `core/src/state/*` |
| Map symbol registry | In progress | Source map rows for `core/src/state/program_state.rs` |
| Map standard token contracts | In progress | `contracts/mt20_token/src/lib.rs` source row |
| Map wrapped token contracts | In progress | Wrapped token source rows |
| Map launchpad and DEX surfaces | In progress | Launchpad and DEX source rows |
| Record doc/source drift | In progress | `getTokenBalance` param order and wrapped decimals drift rows |

Exit criteria:

- Every token exchange-facing method has exact params, return shape, error behavior, and pagination documented.
- Every current drift row has a disposition: fix now, block readiness, or document as intentional.
- Source map is complete enough to write external token guides without guessing.

### Phase 1: Metadata Model And Listing Policy

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Define per-token listing manifest schema | Open | Planned `docs/guides/TOKEN_EXCHANGE_METADATA.md` |
| Define source-of-truth priority | Open | Must cover deployed state, signed manifest, registry, source code, docs |
| Define token class labels | Drafted | Plan class table |
| Define decimal validation vectors | Open | Required before wrapped token package |
| Define registry correction policy | Open | Required before external publication |

Exit criteria:

- Token listing package cannot be produced without a complete manifest.
- Metadata conflict fails readiness.
- Decimals are validated with raw-unit examples.

### Phase 2: Token Exchange Docs And Developer Portal

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Create GitHub token integration guide | Open | Planned `docs/guides/TOKEN_EXCHANGE_INTEGRATION.md` |
| Create developer portal token integration page | Open | Planned `developers/token-exchange-integration.html` |
| Add portal navigation | Open | Must keep native LICN and token docs distinct |
| Add readiness warnings | Open | Testnet-only and per-token-only scope required |
| Add public page audit/readiness checks | Open | Planned frontend/readiness gate updates |

Exit criteria:

- GitHub docs and developer portal content are aligned.
- Developer portal is inline and externally usable.
- Native LICN page does not imply token readiness.

### Phase 3: RPC, Index, Archive, And History Tests

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Test `getTokenTransfers` hot/cold/reopen | Open | Required Rust/RPC tests |
| Test `getTokenBalance` after restart | Open | Required Rust/RPC tests |
| Test `getTokenAccounts` after restart | Open | Required Rust/RPC tests |
| Test `getTokenHolders` pagination and limits | Open | Required Rust/RPC tests |
| Test transfer-to-transaction correlation | Open | Required RPC/e2e tests |
| Test malformed token/holder params | Open | Required RPC tests |

Exit criteria:

- Deposit detection path is proven with transfer history and transaction lookup.
- Current-state versus archive-backed behavior is explicitly documented.
- Public testnet verification can be automated.

### Phase 4: Token Withdrawal Construction

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Identify canonical token transfer instruction format | Open | Source and CLI/SDK inspection required |
| Document token withdrawal cookbook | Open | Planned token guide |
| Verify CLI token transfer path | Open | Required command evidence |
| Verify SDK token transfer builders | Open | Required Rust/JS/Python evidence |
| Test duplicate-broadcast prevention | Open | Required simulation evidence |
| Document native fee funding | Open | Planned token guide |

Exit criteria:

- Exchange can build and broadcast token withdrawals without private help.
- Retry and idempotency behavior is tested.
- Raw integer amounts are preserved end to end.

### Phase 5: Contract Risk And Admin Controls

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Create contract risk sheet template | Open | Planned token operations docs |
| Document admin/minter/pauser roles | Open | Required per-token |
| Verify `getContractInfo` output | Open | Required per-token |
| Verify ABI and code hash | Open | Required per-token |
| Document pause/upgrade/emergency policy | Open | Planned operations pack |
| Define listing rejection criteria | Open | Planned policy |

Exit criteria:

- Exchanges can see mutability and admin risk before listing.
- Token package includes ABI, code hash, owner/admin, pause state, and emergency controls.

### Phase 6: Wrapped Asset, Reserve, And Custody Package

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Reconcile wrapped decimals | Open | `docs/defi/WRAPPED_ASSETS.md` drift row |
| Document reserve model | Open | Planned operations pack |
| Document custody mint/burn/redemption flow | Open | Planned operations pack |
| Verify reserve/liability accounting | Open | Required simulation |
| Verify proof/attestation paths | Open | Required tests/evidence |
| Document reserve incident policy | Open | Planned operations pack |

Exit criteria:

- Wrapped token package includes reserve proof, custody status, and redemption policy.
- Mint/burn and reserve/liability simulation is green.
- Decimals and raw units are unambiguous.

### Phase 7: Launchpad And DEX Token Policy

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Define launchpad maturity thresholds | Open | Planned token guide |
| Define graduated-token policy | Open | Planned token guide |
| Define liquidity and holder requirements | Open | Planned token guide |
| Verify DEX pair/pool IDs | Open | Required per-token |
| Define market-integrity review | Open | Planned operations pack |
| Define abuse/fraud response | Open | Planned operations pack |

Exit criteria:

- Launchpad and DEX tokens cannot be packaged before maturity and liquidity gates pass.
- Exchange docs distinguish trading context from custody/accounting readiness.

### Phase 8: Local Token Exchange Simulation

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Implement standard token simulation | Open | Planned `scripts/qa/token_exchange_simulation.py` |
| Run clean local four-validator simulation | Open | Future evidence artifact |
| Run restart/rejoin drill | Open | Future evidence artifact |
| Run archive migration/reopen drill | Open | Future evidence artifact |
| Add wrapped token variant | Open | Future evidence artifact |
| Add launchpad/DEX variant when scoped | Open | Future evidence artifact |
| Verify cleanup | Open | Future evidence artifact |

Exit criteria:

- Local simulation is green before public testnet.
- Restart/rejoin and hot/cold history behavior are green.
- Cleanup is verified.

### Phase 9: Public Testnet Token Verification

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Run public testnet token simulation | Open | Future evidence artifact |
| Verify public RPC and WebSocket | Open | Future readiness report |
| Verify explorer token routes | Open | Future readiness report |
| Verify developer portal token page | Open | Future readiness report |
| Verify status page and contacts | Open | Future readiness report |
| Verify testnet chain health after run | Open | Future readiness report |

Exit criteria:

- Public testnet token package is green.
- No mainnet claim is made.
- Public readiness gate fails closed until status approval and package tag selection.

### Phase 10: External Token Listing Package

Status: open.

| Task | Status | Evidence |
| --- | --- | --- |
| Publish signed per-token manifest | Open | Future package tag |
| Publish token exchange guide | Open | Future docs/portal |
| Publish token operations pack | Open | Future docs/portal |
| Publish validation report | Open | Future evidence report |
| Select package tag | Open | Future release/tag |
| Record mainnet deferral or mainnet readiness | Open | Future readiness report |

Exit criteria:

- Per-token package is signed and published.
- CI and readiness gates are green.
- Operator approval is recorded.

## Per-Token Package Template

Each token added later must add one row here and a linked package file or manifest.

| Token | Class | Network scope | Contract/program ID | Decimals source | Status | Package |
| --- | --- | --- | --- | --- | --- | --- |
| None yet | - | - | - | - | Not ready | - |

Required per-token statuses:

- `Draft`: metadata being collected.
- `Blocked`: source, docs, tests, reserve, admin, or operator issue prevents listing.
- `Local green`: local validator simulation passed.
- `Public testnet green`: public testnet simulation and readiness gate passed.
- `Package published`: external testnet package published.
- `Mainnet green`: only after mainnet launch handoff and full mainnet token readiness pass.

## Verification Log

| Date | Check | Result |
| --- | --- | --- |
| 2026-07-02 | Token plan and tracker created | Docs-only start; no code or deployment changes |
| 2026-07-02 | Initial token source map | Token RPC, symbol registry, token indexes, wrapped-token constants, launchpad REST, and wrapped docs drift identified |

## Current Blocking Summary

Future Lichen token listings are not fully packaged. Before claiming readiness, the next ordered work is:

1. Finish Phase 0 source map and drift disposition.
2. Define the per-token metadata manifest and source-of-truth policy.
3. Write the token exchange integration guide and developer portal page.
4. Add token RPC/index/archive tests and exact token transaction examples.
5. Build and run the local token exchange simulation on four validators, with restart/rejoin and cleanup.
6. Run public testnet token simulation and public readiness gate.
7. Publish a signed per-token external package.

Until those gates close, only native LICN is covered by the existing exchange package.
