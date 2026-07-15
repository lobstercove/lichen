# Lichen Exchange Listing Readiness Tracker

**Created:** 2026-06-29
**Plan:** [EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md](./EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md)
**Current rollback anchor:** `v0.5.221` per operator update on 2026-07-01
**Exchange package tag:** `exchange-testnet-v0.5.221`
**Current phase:** Phase 8 complete for the current testnet-only exchange package; mainnet remains deferred until mainnet launch handoff
**Rule:** Do not present this package as mainnet-ready, and do not publish any mainnet exchange package until the mainnet launch handoff and full-scope readiness gate pass.
**2026-07-02 correction:** Internal operator monitoring is admin-only and must not be published as the exchange status page.
**2026-07-05 status:** `https://exchanges.lichen.network` is active on Cloudflare Pages, uses exchange-safe status content and a same-origin read-only status RPC proxy, and passed the default public readiness gate for the current testnet-only package.

## Gate Status

| ID | Gate | Status | Release blocker | Evidence / source |
| --- | --- | --- | --- | --- |
| P0-01 | Source map completed | Done | No | This tracker, created from source inspection on 2026-06-29 |
| P0-02 | Version drift documented | Done | Yes | Version drift table below |
| P0-03 | Version drift resolved for core docs and Rust SDK pin | Done | No | README, mainnet/production runbooks, RPC docs, easy-node docs, Rust SDK pin and lockfile |
| P0-04 | Chain metadata source map completed | Done | No | Chain metadata source map below |
| P0-05 | Chain metadata final values verified | Done | No | Explorer routes, logo, `v0.5.221` rollback-anchor signatures, testnet runtime fee, testnet RPC/WS readiness, current testnet-only scope, EVM wording, raw-spores accounting guidance, approved incident aliases, public developer-page deployment, active public exchange status page, CI, and package release `exchange-testnet-v0.5.221` are checked |
| P1-01 | Exchange integration guide skeleton | Done | No | `docs/guides/EXCHANGE_INTEGRATION.md`, `developers/exchange-integration.html`; published for testnet-only package |
| P1-02 | Dedicated checklist reviewed | Done | No | This tracker; technical checklist rows are green for testnet-only scope |
| P1-03 | Chain metadata sheet skeleton | Done | No | `docs/guides/EXCHANGE_CHAIN_METADATA.md`; published for testnet-only package |
| P1-04 | Operations pack skeleton | Done | No | `docs/deployment/EXCHANGE_OPERATIONS_PACK.md`; published for testnet-only package |
| P1-05 | Developer portal exchange page aligned | Done | No | `bash scripts/deploy-cloudflare-pages.sh developers` redeployed the page on 2026-07-04; the live page carries inline exchange metadata, deposit/withdrawal cookbooks, finality/archive policy, operations contacts, validation gates, mainnet handoff, release-tagged source links, the exchange status URL without the old planned wording, and no admin monitoring URL |
| P1-06 | Native address validation vectors documented and tested | Done | No | `docs/guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md`, `core/src/account.rs` test |
| P2-01 | Finality policy validated locally | Done | No | Local three-validator stack on 2026-06-29; evidence section below |
| P2-02 | Finality behavior checked across validator restart | Done | No | V2 child process restarted under supervisor; all validators reconverged |
| P3-01 | Archive/history behavior validated locally | Done | No | Core and RPC archive regressions passed on 2026-06-29; evidence section below |
| P4-01 | Local exchange simulation implemented | Done | No | `scripts/qa/exchange_simulation.py` |
| P4-02 | Local exchange simulation passed from clean three-validator stack | Done | No | Phase 4 evidence below |
| P4-03 | Local cleanup verified | Done | No | Stop/status/process checks and generated state cleanup recorded below |
| P5-01 | CLI examples verified | Done | No | CLI balance, transfer, account history, and tx lookup passed during exchange simulation |
| P5-02 | SDK compatibility boundary verified | Done | No | Rust check passed; JS build passed with documented integer-precision boundary; Python wrapper test passed |
| P5-03 | Explorer URL patterns verified | Done | No | Source route inspection plus hosted root/account/transaction/block `200` checks on 2026-06-29 |
| P6-01 | Native LICN integration separated from DEX/custody/wrapped-asset context | Done | No | Phase 6 evidence below; native guide excludes DEX/custody/oracle from base deposit flow |
| P7-01 | Public testnet exchange run passed | Done | No | Signed `v0.5.221` deployed through the runbook; public RPC/WS/faucet/DEX smoke passed; public faucet-backed exchange simulation passed and wrote `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json` |
| P8-01 | External listing package reviewed | Done | No | Technical package gates are green for testnet scope, incident aliases are approved, package release `exchange-testnet-v0.5.221` is published, `https://exchanges.lichen.network` is active and passed default readiness, and mainnet is deferred until mainnet launch handoff |

## Phase 0 Source Map

| Area | Source-backed fact | Source files |
| --- | --- | --- |
| Native account format | Native account IDs are 32-byte `Pubkey` values. Native display format is Base58 encoding of exactly those 32 bytes. Parsing rejects non-Base58 strings and decoded lengths other than 32 bytes. | `core/src/account.rs` (`Pubkey`, `to_base58`, `from_base58`) |
| PQ account derivation | ML-DSA-65 public keys derive account addresses as `scheme_version` byte plus the first 31 bytes of `SHA-256(public_key_bytes)`. | `core/src/account.rs` (`PqPublicKey::address`) |
| Native keypair | Native signing uses ML-DSA-65 keypairs; the account address is derived from the PQ public key. | `core/src/account.rs` (`Keypair`, `Keypair::pubkey`, `Keypair::sign`) |
| EVM display mapping | `Pubkey::to_evm()` hashes the 32-byte native pubkey with Keccak-256 and returns the last 20 bytes as `0x...`. This is a mapping/display surface, not the native deposit address format. | `core/src/account.rs`, `core/src/evm.rs` |
| EVM compatibility chain ID | Native exchange integrations use string chain IDs such as `lichen-mainnet-1` and `lichen-testnet-1`. `/evm` `eth_chainId` is a runtime compatibility value derived by `rpc/src/lib.rs`; public testnet returned `0xca3f1595a6c25e9f` on 2026-06-30. `core/src/evm.rs` `LICHEN_CHAIN_ID = 8001` is a compatibility/default constant, not the native LICN listing chain ID. | `core/src/evm.rs`, `rpc/src/lib.rs`, public testnet `eth_chainId` |
| Unit model | Account balances are stored as `u64` spores. `Account::licn_to_spores(1) = 1_000_000_000`; `spores_to_licn` truncates fractional LICN. | `core/src/account.rs` |
| Fee model | Default genesis base fee is `1_000_000` spores, or `0.001 LICN`, before any priority fee or instruction premium. Current fee config is queryable by `getFeeConfig`. | `core/src/genesis.rs`, `core/src/processor/fees.rs`, `rpc/src/lib.rs` |
| Transaction ID | Native transaction ID is `SHA-256` of the serialized signed transaction envelope. | `core/src/transaction.rs` (`Transaction::hash`) |
| Transaction signing domain | Chain-aware native signing wraps message bytes with `LICHEN-SIG`, version, domain, chain ID, and payload length. | `core/src/signing.rs`, `core/src/transaction.rs` |
| Core RPC routing | Canonical JSON-RPC is rooted at `/`; compatibility routes are `/solana-compat` and `/evm`. | `rpc/src/lib.rs` |
| Transfer submission | Canonical native writes use `sendTransaction` with base64 transaction bytes; EVM-typed txs are rejected by this native path. | `rpc/src/lib.rs` (`handle_send_transaction`, `preflight_transaction_submission`) |
| Balance RPC | `getBalance` returns raw spores and formatted LICN strings. The formatted LICN strings currently use four decimal places, so exchange accounting must use raw spores. | `rpc/src/lib.rs` (`handle_get_balance`) |
| Block RPC | `getBlock(slot)` reads canonical slot-to-hash mapping and returns full block transaction JSON; `getLatestBlock` returns a block summary for the last slot. | `rpc/src/lib.rs`, `core/src/state/ledger_state.rs` |
| Transaction RPC | `getTransaction(signature)` reads transaction bodies by hash, requires or discovers the inclusion slot, and adds `confirmation_status` / `confirmations`. | `rpc/src/lib.rs`, `core/src/state/ledger_state.rs`, `core/src/state/secondary_indexes.rs` |
| Account history RPC | `getTransactionsByAddress` uses account transaction indexes, newest first, with `limit` and `before` or `before_slot` cursor pagination. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` |
| Account tx count | `getAccountTxCount` calls `count_account_txs`; when cold storage is attached, hot and cold account indexes are deduplicated by key. | `rpc/src/lib.rs`, `core/src/state/secondary_indexes.rs` |
| Archive mode | Archive mode records account snapshots; cold store falls through for blocks, transactions, tx-to-slot, and account transaction indexes when attached. | `core/src/state/archive_state.rs`, `core/src/state/cold_storage.rs`, `core/src/state/ledger_state.rs`, `core/src/state/secondary_indexes.rs` |
| Public history repair | Public-history merge supports full and index-only modes and refuses non-dry-run merges without an attached cold store when cold-backed targets are required. | `core/src/state/cold_storage.rs` |
| Finality model | `FINALITY_DEPTH` is currently `0`; when a slot is marked confirmed, finalized advances to the same slot. RPC commitment status is read from `FinalityTracker` when present. | `core/src/consensus.rs`, `rpc/src/lib.rs`, `rpc/src/ws.rs` |
| Local e2e stack | Local production-parity stack starts a seed, custody/faucet/source-chain mocks, then joiners from sync. Default reset behavior is enabled by `LICHEN_LOCAL_RESET_CLUSTER=1`. | `scripts/start-local-stack.sh`, `scripts/start-local-3validators.sh` |
| Local cleanup | Local stack cleanup stops validators, custody, faucet, and local source-chain mocks. | `scripts/stop-local-stack.sh`, `scripts/status-local-stack.sh` |
| CLI transfer | `lichen transfer <to> <amount>` loads a keypair, parses Base58 destination, converts LICN f64 input to spores, and submits a native transfer. | `cli/src/cli_args.rs`, `cli/src/transfer_support.rs`, `cli/src/client_native_write_support.rs` |
| CLI account history | `lichen account history <address> --limit <n>` calls `getTransactionsByAddress`. | `cli/src/cli_args.rs`, `cli/src/account_support.rs`, `cli/src/client_transaction_query_support.rs` |
| Custody service | Custody is a separate REST service with `/health`, `/status`, `/deposits`, `/withdrawals`, reserves, webhooks, and event streams. This is bridge/wrapped-asset infrastructure, not required for native LICN exchange deposits. | `custody/src/bootstrap_support/router.rs`, `docs/guides/CUSTODY_PLAN.md` |
| DEX surface | DEX REST is read-heavy and exposes pairs, orderbooks, routes, pools, margin, rewards, governance, stats, and oracle prices under API routes. Writes are rejected or routed through signed transactions, not raw REST mutations. | `rpc/src/dex.rs`, `contracts/dex_core/src/lib.rs`, `contracts/dex_amm/src/lib.rs` |
| Oracle surface | Oracle contract has owner/feeder controls, price submission, aggregation, pause/resume, and stats. Runtime also has native consensus oracle paths elsewhere; exchange docs must avoid overstating contract oracle as the only authority. | `contracts/lichenoracle/src/lib.rs`, `core/src/processor/governance_oracle.rs`, `rpc/src/dex.rs` |

## Version Drift

These are release-blocking inconsistencies for any exchange-facing package.

| Component | Observed value | Source | Status |
| --- | --- | --- | --- |
| Core crate | `0.5.221` | `core/Cargo.toml` | Current signed testnet recovery release |
| RPC crate | `0.5.221` | `rpc/Cargo.toml` | Current signed testnet recovery release |
| Validator crate | `0.5.221` | `validator/Cargo.toml` | Current signed testnet recovery release |
| CLI crate | `0.5.221` | `cli/Cargo.toml` | Current signed testnet recovery release |
| Root README release text | `v0.5.221` with `v0.5.221` rollback anchor | `README.md` | Updated |
| Mainnet runbook release text | `v0.5.221` release target and rollback anchor; mainnet is not live and is not part of the testnet recovery release | `docs/deployment/MAINNET_LAUNCH_RUNBOOK.md` | Updated |
| Production deployment runbook release text | `v0.5.221` current recovery release with `v0.5.221` rollback anchor | `docs/deployment/PRODUCTION_DEPLOYMENT.md` | Updated |
| RPC API docs version | `0.5.215` pending exchange-doc package refresh | `docs/guides/RPC_API_REFERENCE.md` | Not part of the current recovery patch |
| Rust SDK package | `0.1.6` | `sdk/rust/Cargo.toml` | Candidate; publish only with `v0.5.224` after all release gates pass |
| Rust SDK core dependency | `=0.5.224` plus local core path | `sdk/Cargo.toml`, `sdk/rust/Cargo.toml` | Candidate; locked SDK tests and package-content checks passed |
| JS SDK package | `1.0.6` | `sdk/js/package.json`, `sdk/js/README.md` | Candidate; build and lossless archive-accounting gates must pass before publish |
| Python SDK package | `1.0.0` | `sdk/python/pyproject.toml`, `sdk/python/README.md` | Exact JSON integer path; archive wrappers added and tested |

## Chain Metadata Source Map

This table names the source of truth. It is not yet the final external metadata sheet.

| Field | Current source-backed value or rule | Source of truth | Status |
| --- | --- | --- | --- |
| Chain name | Lichen | Product docs and package metadata | Needs final metadata sheet |
| Native ticker | `LICN` | Foundation/tokenomics docs, RPC examples, SDK metadata | Needs final metadata sheet |
| Native decimals | `9` | `Account::licn_to_spores`, token docs, RPC examples | Source mapped |
| Base unit | `spore`; `1 LICN = 1,000,000,000 spores` | `core/src/account.rs` | Source mapped |
| Fee unit | Native LICN spores | `core/src/processor/fees.rs`, `core/src/genesis.rs` | Source mapped |
| Default base fee | `1,000,000` spores (`0.001 LICN`) | `core/src/genesis.rs`, `getFeeConfig` at runtime | Public testnet runtime value verified after signed `v0.5.221` recovery rollout on 2026-07-01: `base_fee_spores = 1000000` |
| Native mainnet chain ID | `lichen-mainnet-1` | `seeds.json`, `core/src/network.rs` | Source mapped |
| Native testnet chain ID | `lichen-testnet-1` | `seeds.json`, `core/src/network.rs` | Source mapped |
| EVM compatibility chain ID | Query `/evm` `eth_chainId` at runtime; live public testnet returned `0xca3f1595a6c25e9f` on 2026-06-30. `8001` is a core compatibility/default constant and must not be published as the native LICN listing chain ID. | `core/src/evm.rs`, `rpc/src/lib.rs`, public testnet `eth_chainId` | Source mapped and wording reconciled |
| Native address validation | Decode Base58; decoded byte length must be exactly 32. Regex prefilter: `^[1-9A-HJ-NP-Za-km-z]{32,44}$`. Regex alone is insufficient. | `core/src/account.rs`, `docs/guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md` | Source mapped and tested |
| EVM address mapping | `0x` plus Keccak-derived 20-byte address from native 32-byte pubkey | `core/src/account.rs` | Source mapped |
| Mainnet RPC URL | `https://rpc.lichen.network` | `seeds.json`, `developers/shared-config.js`, `core/src/network.rs`, mainnet launch runbook | Launch placeholder; excluded from current testnet-only package until mainnet launch handoff passes |
| Mainnet WebSocket URL | `wss://rpc.lichen.network/ws` | `developers/shared-config.js`, deployment docs, mainnet launch runbook | Launch placeholder; excluded from current testnet-only package until mainnet launch handoff passes |
| Testnet RPC URL | `https://testnet-rpc.lichen.network` | `seeds.json`, `developers/shared-config.js`, `core/src/network.rs` | Healthy after signed `v0.5.221` recovery rollout; sustained public cadence sampled `370.0ms/block`, `getMetrics.observed_block_interval_ms = 372`, and `avg_block_time_ms = 380` |
| Testnet WebSocket URL | `wss://testnet-rpc.lichen.network/ws` | `developers/shared-config.js`, deployment docs | Public readiness WebSocket check passed after signed `v0.5.221` recovery rollout; live slot notifications advanced `6871609` -> `6871611` |
| Explorer URL | `https://explorer.lichen.network` | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js` | Verified: public templates are `/address?address=...`, `/transaction?sig=...`, and `/block?slot=...` |
| Logo asset | Public asset: `https://lichen.network/Lichen_Logo_256.png`; repo asset exists at `website/Lichen_Logo_256.png` | `website/`, deployed site config | Verified public PNG: 256x256, 45,415 bytes, SHA-256 matches repo asset |
| Public exchange status page | `https://exchanges.lichen.network` | Operations pack; `exchanges/` frontend; Cloudflare Pages | Project `lichen-network-exchanges` redeployed on 2026-07-05; custom domain is active; same-origin `/api/rpc` returned public testnet `getHealth.status = ok`; default readiness is green |
| Release signer | `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` | `deploy/release-trust-anchor.json` | Source mapped |
| Release signatures | `SHA256SUMS.sig` signed by release signer; verification via `scripts/verify-release-checksums.mjs` | `scripts/sign-release.sh`, `scripts/verify-release-checksums.mjs`, `.github/workflows/release.yml` | Verified for rollback anchor and current signed testnet recovery tag `v0.5.221`; final external docs package still required |

## Open Phase 0 Blockers

No open Phase 0 blockers remain for the current testnet-only exchange package.
Mainnet remains deferred until the mainnet launch exchange handoff gate and
full-scope readiness gate pass.

## Resolved Blockers

| ID | Former blocker | Resolution | Evidence |
| --- | --- | --- | --- |
| B0-11 | Final external exchange package was not published | Published testnet-only exchange package under `exchange-testnet-v0.5.221` | `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221`; package assets `lichen-exchange-testnet-v0.5.221.tar.gz` and `SHA256SUMS` |
| B0-10 | Incident/contact aliases were not approved for exchange use | Operator approved `security@lichen.network`, `exchange-ops@lichen.network`, and `business@lichen.network` on 2026-07-01 with acknowledgement/update/maintenance policy recorded in the operations pack | `docs/deployment/EXCHANGE_OPERATIONS_PACK.md` |
| B0-08 | Status page was not operator-approved as the exchange status page | Superseded on 2026-07-02: internal monitoring is admin-only and must not be used as the public exchange status page | Current open blocker is B0-17 |
| B0-01 | Release docs disagreed with the then-current operator anchor `v0.5.215` and signed recovery release `v0.5.219` | Superseded by the 2026-07-01 operator update: README, mainnet runbook, production runbook, exchange docs, and readiness gate now use `v0.5.221` as the rollback anchor | Targeted version scan completed after the `v0.5.221` anchor update |
| B0-02 | Rust SDK dependency pinned `lobstercove-lichen-core = "=0.5.207"` while protocol crates moved to `0.5.219` | Updated Rust SDK manifest and lockfile to `0.5.219` | `cargo check --manifest-path sdk/rust/Cargo.toml` |
| B0-03 | RPC docs reported version `0.5.178` | Updated RPC API reference version and sample network-info response to `0.5.215` | `docs/guides/RPC_API_REFERENCE.md` |
| B0-04 | Address regex not backed by generated vectors | Added source-derived valid/invalid vectors and a focused core regression test | `docs/guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md`; `cargo test -p lobstercove-lichen-core account::tests::test_exchange_address_validation_vectors` |
| B0-07 | Finality policy not operationally validated under restart/lag | Validated processed/confirmed/finalized reporting, sampled finalized transaction status, plus-8/plus-32 buffer, V2 restart convergence, and cleanup on a local three-validator stack | Commands/results recorded in Phase 2 Evidence below |
| B0-06 | `getTransaction` archive behavior was source-mapped but not verified through hot/cold migration and restart/reopen | Added core and RPC regressions that migrate block bodies, transaction bodies, tx-to-slot, and account transaction indexes to cold storage, reopen state, then verify archive-backed exchange lookup methods | `cargo test -p lobstercove-lichen-core state::tests::exchange_archive_history_survives_hot_to_cold_migration_and_reopen`; `cargo test -p lichen-rpc test_native_archive_history_rpc_survives_cold_migration_and_reopen` |
| B0-11 | Explorer URL patterns were not confirmed | Verified source route handling and hosted public templates for address, transaction, and block pages | `explorer/js/address.js`, `explorer/js/transaction.js`, `explorer/js/block.js`; hosted route checks recorded in Phase 5 metadata evidence |
| B0-12 | Logo URL was not confirmed | Verified public `https://lichen.network/Lichen_Logo_256.png` is a 256x256 PNG and byte-identical to `website/Lichen_Logo_256.png` | SHA-256 `bfa0986bc4bde64c3c7ce590782beba78980985f301fbd0fbd4a39dc045ca876` |
| B0-13 | Rollback release signature URLs were not confirmed | Verified `v0.5.221` GitHub release, checksum/signature assets, and PQ signature against the trust anchor signer | `scripts/verify-release-checksums.mjs` against downloaded `v0.5.221` release artifacts |
| B0-15 | Public testnet exchange run was pending | Signed `v0.5.219` was deployed and verified through the runbook; public RPC/WS/faucet/DEX smoke passed; public faucet-backed exchange simulation passed | `tests/artifacts/exchange-simulation-public-testnet-v0.5.219.json`; `docs/deployment/TESTNET_RECOVERY_INCIDENT_2026-06-30.md` |
| B0-09 | Mainnet public RPC/WS scope was unresolved for the current package | Package is explicitly testnet-only until mainnet launch. Mainnet RPC/WS readiness is deferred to the mainnet launch exchange handoff gate and must pass `exchange_public_readiness.py --scope full` before mainnet is included | `docs/guides/EXCHANGE_INTEGRATION.md`; `docs/guides/EXCHANGE_CHAIN_METADATA.md`; `docs/deployment/MAINNET_LAUNCH_RUNBOOK.md` |
| B0-16 | EVM chain ID wording was ambiguous for native listings | Native exchange integrations now use `getNetworkInfo.chain_id`; EVM compatibility uses runtime `/evm` `eth_chainId`; `8001` is documented only as a compatibility/default constant | `docs/guides/EXCHANGE_INTEGRATION.md`; `docs/guides/EXCHANGE_CHAIN_METADATA.md` |
| B0-05 | `getBalance` formatted LICN string only has four decimals | Exchange guide requires raw `u64` spores for all balances, credits, withdrawals, fee accounting, and reconciliation; formatted LICN strings are documented as display-only and not suitable for exchange accounting | `docs/guides/EXCHANGE_INTEGRATION.md` |
| B0-14 | Public developer portal exchange page was stale | Deployed `developers/` to Cloudflare Pages project `lichen-network-developers`; ten public checks returned HTTP `200` with `testnet-only`, `mainnet launch exchange handoff`, and `Exchange Operations Pack`; post-deploy public readiness check passed the developer page gate | `bash scripts/deploy-cloudflare-pages.sh developers`; `python3 scripts/qa/exchange_public_readiness.py --scope testnet --report /tmp/lichen-exchange-public-readiness-scope-testnet-post-deploy.json` |

## Phase 2 Evidence

Local finality validation ran on 2026-06-29 against a clean local `testnet`
three-validator stack.

| Check | Result |
| --- | --- |
| Start command | `scripts/start-local-stack.sh testnet` |
| Startup result | Three validators reached the same slot/hash at startup; custody, faucet, and local source-chain mocks started |
| Health check | `scripts/status-local-stack.sh testnet` showed RPC ports `8899`, `8901`, and `8903` healthy |
| Commitment slots | `getSlot` with processed, confirmed, and finalized returned `138` on all three validators during first check |
| Latest block convergence | `getLatestBlock` returned slot `138`, hash `01209a1f793cf011d6d5f12f7156f49c578df018ab06c25927388a6ec3c22b58`, and transaction count `0` on all three validators |
| Sample transaction | Slot `169`, signature `b1187403f1ad94c7cc353615cdb7c2b726a92687721dbf6d99799811c143936c` |
| Transaction commitment | `getTransaction` returned slot `169`, `confirmation_status = finalized`, and `confirmations = null` on all three validators |
| Buffer check | Finalized slots were `231`, `232`, `232`; transaction slot `169` passed both plus-8 and plus-32 checks |
| Restart check | V2 child PID `54458` was killed; supervisor restarted it as PID `61137` after 3 seconds |
| Post-restart convergence | All validators reported finalized/latest slot `427` with hash `578c412b6e45af9ec7a6ad4825a980f58f6a81b094b8e5021d3219e354b762ae` |
| Post-restart transaction lookup | The sampled transaction still returned finalized/null confirmations on all three validators |
| Cleanup | `scripts/stop-local-stack.sh testnet` reported validators, custody, faucet, local EVM RPC, and local Solana RPC stopped; follow-up status showed all local services down |

## Phase 3 Evidence

Archive/history validation ran on 2026-06-29 against local hot and cold
RocksDB stores with restart/reopen coverage.

| Check | Result |
| --- | --- |
| Core storage regression | `state::tests::exchange_archive_history_survives_hot_to_cold_migration_and_reopen` passed |
| RPC archive regression | `test_native_archive_history_rpc_survives_cold_migration_and_reopen` passed |
| Data path covered | `put_block_atomic` wrote transaction body, signature-to-slot, tx-by-slot, canonical slot, and account history indexes |
| Migration covered | `migrate_to_cold(15)` moved the older block, transaction body, and tx-to-slot index; `migrate_indexes_to_cold(15)` moved older account history rows |
| Restart/reopen covered | Hot and cold stores were closed, reopened, and reattached before final assertions |
| Core methods verified | `get_block_by_slot`, `get_transaction`, `get_tx_slot`, `get_account_tx_signatures_paginated`, and `count_account_txs` returned old cold-backed and newer hot-backed data |
| RPC methods verified | `getBlock`, `getTransaction`, `getTransactionsByAddress`, and `getAccountTxCount` returned archived data after reopen |

## Phase 5 SDK Evidence

SDK compatibility validation ran on 2026-06-29.

| Check | Result |
| --- | --- |
| Rust SDK | Workspace release checks passed after pinning `lobstercove-lichen-core = "=0.5.221"` |
| JavaScript SDK | `npm run build` passed in `sdk/js` |
| JavaScript exchange boundary | `sdk/js/README.md` now states the SDK is not approved for exchange accounting because native JSON parsing cannot preserve all u64 spore values |
| JavaScript archive helpers | `getTransactionsByAddress`, `getAccountTxCount`, `getTransaction`, and `getBlock` cover the canonical archive surface |
| Python SDK | `./.venv/bin/python -m pytest sdk/python/test_connection_cleanup.py -q` passed |
| Python archive helpers | `get_transactions_by_address`, `get_account_tx_count`, `get_transaction`, and `get_block` cover the canonical archive surface |

## Phase 5 Explorer And Metadata Evidence

Explorer and public metadata checks ran on 2026-06-29 from outside the local
validator stack.

| Check | Result |
| --- | --- |
| Explorer source route handling | `address.js` reads `address` and `addr`; `transaction.js` reads `sig`, `tx`, `hash`, and `signature`; `block.js` reads `slot` and `block` |
| Exchange-facing explorer templates | Use `https://explorer.lichen.network/address?address={address}`, `https://explorer.lichen.network/transaction?sig={signature}`, and `https://explorer.lichen.network/block?slot={slot}` |
| Hosted explorer root | `https://explorer.lichen.network/` returned HTTP `200` |
| Hosted address route | `https://explorer.lichen.network/address?address=7YKDTkwQWmDx9auTwhAJMVEkBdmFPeeE485dgM5fHxy` returned HTTP `200` |
| Hosted transaction route | `https://explorer.lichen.network/transaction?sig=c99c0b7f1b984cf48773080fbdc72c834431625eae8e2c340ec3d435498c4bd0` returned HTTP `200` |
| Hosted block route | `https://explorer.lichen.network/block?slot=1` returned HTTP `200` |
| Static `.html` route behavior | Hosted `.html` route checks redirected with HTTP `308` to extensionless public routes, then returned HTTP `200` |
| Developer portal exchange page | `https://developers.lichen.network/exchange-integration` returned HTTP `200` after Cloudflare Pages deployment and contains the testnet-only exchange package content directly: metadata, address/accounting rules, deposit and withdrawal cookbooks, finality/archive policy, operations contacts, validation gates, mainnet handoff, and release-tagged source links |
| Public logo URL | `https://lichen.network/Lichen_Logo_256.png` returned HTTP `200`, `image/png`, 45,415 bytes, cache max-age 14,400 |
| Public logo file | Downloaded PNG is 256x256 RGBA; SHA-256 `bfa0986bc4bde64c3c7ce590782beba78980985f301fbd0fbd4a39dc045ca876`, matching `website/Lichen_Logo_256.png` |
| Public exchange status page | Active after the 2026-07-05 deploy: `https://exchanges.lichen.network` is the exchange-safe Cloudflare Pages status page, the custom domain is active, `/api/rpc` returned public testnet `getHealth.status = ok`, and default public readiness passed; internal operator monitoring remains admin-only and must not be published |
| Testnet RPC health before recovery | `https://testnet-rpc.lichen.network` responded to `health` with `status = behind`, `reason = stale_tip`, slot `6708256`, and `block_age_secs = 22561` |
| Testnet RPC health after final rollout | `https://testnet-rpc.lichen.network` responded to `getHealth` with `status = ok`, `reason = ok`, slot `6764490`, and `block_age_secs = 0` after signed `v0.5.219` rollout; a later sample returned slot `6772418`, `block_age_secs = 0` |
| Testnet runtime fee query | `getFeeConfig` on public testnet passed after signed `v0.5.219` rollout with `base_fee_spores = 1000000` |
| Testnet WebSocket | `wss://testnet-rpc.lichen.network/ws` accepted `subscribeSlots` and returned a subscription id after signed `v0.5.219` rollout |
| Mainnet RPC | `https://rpc.lichen.network` is a launch placeholder and excluded from the current testnet-only exchange package; it must be rechecked after mainnet launch with `exchange_public_readiness.py --scope full` |
| Mainnet WebSocket | `wss://rpc.lichen.network/ws` is a launch placeholder and excluded from the current testnet-only exchange package; it must be rechecked after mainnet launch with `exchange_public_readiness.py --scope full` |
| Rollback release page | `https://github.com/lobstercove/lichen/releases/tag/v0.5.221` returned HTTP `200`; GitHub API reports `draft = false` and `prerelease = false` |
| Rollback release assets | API lists Linux, macOS, and Windows validator archives plus `SHA256SUMS` and `SHA256SUMS.sig`; checksum and signature assets downloaded successfully |
| Rollback signature verification | `scripts/verify-release-checksums.mjs` against downloaded `v0.5.221` release artifacts verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` |
| Public readiness gate script | `scripts/qa/exchange_public_readiness.py` writes `tests/artifacts/exchange-public-readiness-report.json`, requires the exchange developer-page content snippets including `Deposit Cookbook`, `Withdrawal Cookbook`, `Canonical JSON-RPC Cookbook`, `Mainnet Handoff`, and `testnet-only`, honors `--scope testnet` versus `--scope full`, checks rollback anchor `v0.5.221`, defaults the status URL to `https://exchanges.lichen.network`, requires status approval, rejects admin monitoring as the exchange status page, and fails closed while blocking public endpoint, developer portal, status-page, or release-selection gates remain open |
| Public readiness gate unit tests | `python3 scripts/qa/test_exchange_public_readiness.py` passed (`14` tests): stale RPC health fails, stale/generic developer page fails, complete developer page passes, developer page with admin monitoring host fails, missing/admin status page fails, public status page passes, Cloudflare-protected contact email decoding passes, admin status content fails, default status URL uses the exchange subdomain, PNG dimension parsing is guarded, testnet/full package scope controls mainnet readiness, and exchange package release assets are enforced |
| Public readiness gate result before recovery | Failed as expected on 2026-06-29 after the stricter check: testnet RPC stale/readiness-gated (`slot = 6708256`, `block_age_secs = 22561`), public developer exchange page content missing, status page not operator-approved, final package tag not selected, and full-scope mainnet RPC/WS readiness not yet available |
| Public readiness gate result after `v0.5.221` recovery deploy | Testnet public RPC health, `getFeeConfig`, finalized-slot, latest-block, WebSocket, explorer, logo, rollback release API, and deployed developer exchange page passed; the remaining public status-page blocker was closed by the 2026-07-05 `https://exchanges.lichen.network` activation |
| Public readiness state on 2026-06-30 | Testnet public/local health was stale at slot `6715444`; signed `v0.5.217` restored liveness, and signed `v0.5.219` completed the clean faucet-signing and exchange-simulation follow-up with all four validators on the signed release |
| Public DNS check | `testnet-rpc.lichen.network`, `rpc.lichen.network`, and `developers.lichen.network` resolve to Cloudflare addresses; dedicated `testnet-ws.lichen.network` and `ws.lichen.network` resolve directly to validator VPS IPs, so the exchange docs continue to advertise only RPC-hosted `/ws` endpoints |
| Testnet VPS service check before recovery | Operator-approved SSH evidence showed all four `lichen-validator-testnet` services active on `v0.5.215` with `--archive-mode --cold-store /var/lib/lichen/archive-testnet`; all four binaries had SHA-256 `e7842eb8533e55e91060ca744e9a130f5fb658062623a3e6940a1f9dd474683e` |
| Testnet VPS health check before recovery | All four local RPC health checks returned `status = behind`, `reason = stale_tip`; three nodes reported slot `6708256`, and `15.204.229.189` reported slot `6707400` |
| Testnet consensus log check before recovery | Logs showed repeated BFT propose/prevote/precommit timeouts, nil polka/commit, and round advances at height `6708257`; `15.204.229.189` had pending far-ahead blocks waiting for missing parents |
| Testnet recovery action | Non-destructive runbook branch used on 2026-06-29: after evidence preservation, only stale validator `15.204.229.189` was restarted with `sudo systemctl restart lichen-validator-testnet`; no state reset, no archive deletion, no RocksDB copy, no release/deploy |
| Testnet recovery result | Five-sample cluster watch showed all four validators recovered to `status = ok` and advanced fresh slots through `6708526`-`6708536`; public testnet `getHealth`, `getFeeConfig`, finalized-slot, latest-block, and WebSocket checks passed |
| Testnet recovery evidence path | Ignored local evidence stored under `evidence/exchange-readiness/live-20260629T154831Z/`, including pre/post RPC snapshots, journals, restart record, cluster watch, public page capture, and public readiness report |
| Developer portal deploy audit | Added exchange-page assertions to the frontend asset audit and public readiness gate; `bash scripts/deploy-cloudflare-pages.sh developers` reruns the audit before deployment so a link-only or stale exchange page fails before outreach |
| Current public testnet scope check | Passed after the 2026-07-05 exchange status-page activation and redeploy. Incident/contact aliases and rollback anchor remain approved, and the readiness command now checks `https://exchanges.lichen.network` by default. |
| Public readiness after exchange status-page correction | Historical 2026-07-04 result: `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-domain-pending.json` failed closed exactly as intended while the custom domain was not active yet. This was superseded by the 2026-07-05 activation and final green report. |
| Exchange status preview readiness | Historical 2026-07-02 result: `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-url https://03e74d4f.lichen-network-exchanges.pages.dev --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-preview-green.json` passed every gate against the deployed Pages preview before the official domain was active. |
| Exchange status Pages readiness after 2026-07-04 redeploy | `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-url https://lichen-network-exchanges.pages.dev --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-pagesdev-green-20260704.json` passed every gate after redeploy. |
| Default exchange status readiness after 2026-07-04 redeploy | Historical 2026-07-04 result: `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-domain-20260704.json` failed closed while `https://exchanges.lichen.network` was not active yet. This was superseded by the 2026-07-05 activation and final green report. |
| Default exchange status readiness after 2026-07-05 activation | `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-domain-20260705-post-status-logic-green.json` passed every gate against `https://exchanges.lichen.network`. |
| Exchange status browser health after 2026-07-05 activation | `https://exchanges.lichen.network/api/rpc` returned public testnet `getHealth.status = ok`; the status page now uses this same-origin read-only proxy so browser CORS does not mark the page degraded. RPC defaults also include `exchanges.lichen.network` for validator rollouts. |
| Exchange and monitoring deploys | `bash scripts/deploy-cloudflare-pages.sh exchanges`, `bash scripts/deploy-cloudflare-pages.sh developers`, and `bash scripts/deploy-cloudflare-pages.sh monitoring` passed predeploy frontend QA and redeployed on 2026-07-04. Current Pages previews: `https://2885630f.lichen-network-exchanges.pages.dev`, `https://77d2a9b7.lichen-network-developers.pages.dev`, and `https://143afa51.lichen-network-monitoring.pages.dev`; `monitoring` keeps the internal Exchange Operations section. |
| Public testnet pace after exchange-docs commit | 45/45 documented public RPC `getHealth` samples succeeded against `https://testnet-rpc.lichen.network`; slots advanced from `6792393` to `6792586`, block age stayed `0-1s`, and estimated cadence was `228.0ms/slot`. Public HTTP latency from the runner location was p50 `460.5ms`, min `452.0ms`, max `935.4ms`; this separates healthy chain cadence from public edge/network latency. |
| Final CI after exchange-docs commit | GitHub Actions CI run `28444514913` for commit `c2ef5ad4873a582c3fd5ad459e70a2fae07e79a1` completed successfully. Green jobs: Prediction Market Tests, Cargo Deny, Expected Contract Lockfile, Test, JS and Python Dependency Health, Cargo Audit, Clippy, Rust SBOM, Wallet Extension, WASM Contract Builds, Format, Docker Build, and Integration Tests. Integration built release validator tooling, started a local validator, and ran RPC integration, CLI integration, deterministic E2E smoke, and full RPC/DEX REST coverage. |
| Final local QA after 2026-07-02 exchange status-page correction | `python3 -m py_compile scripts/qa/exchange_public_readiness.py scripts/qa/test_exchange_public_readiness.py scripts/qa/exchange_simulation.py` passed; `python3 scripts/qa/test_exchange_public_readiness.py` passed `13` tests; `node scripts/qa/test_frontend_asset_integrity.js` passed `374` checks; `git diff --check` passed; private monitoring URL is absent from public exchange docs and portal source. |

## Phase 7 Public Testnet Evidence

Public testnet validation ran on 2026-07-01 after the signed `v0.5.221` recovery rollout.

| Check | Result |
| --- | --- |
| Signed release | `v0.5.221` GitHub release published, not draft, not prerelease |
| Release signature | `SHA256SUMS` and `SHA256SUMS.sig` verified against signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` |
| Runbook deployment | Coordinated signed-release recovery installed `v0.5.221` on all four hosts, stopped/started validators together from preserved state, and did not reset state, copy RocksDB, delete archives/WAL, replace keys, or run clean-slate redeploy |
| Verify-only runbook | Completed `RELEASE VERIFY COMPLETE`; installed validator, custody, and faucet binaries matched signed release archive hashes on all four hosts |
| Live host versions | All four hosts reported `/usr/local/bin/lichen-validator --version = lichen-validator 0.5.221`; local release CLI reported `lichen 0.5.221` |
| Live host health | All four local RPC health checks returned `status = ok`; all four `lichen-faucet.service` units were active |
| Public RPC progression | Post-recovery public cadence advanced from slot `6871769` to `6871959` across 70.39s, estimated `370.0ms/block`; public `getMetrics` returned `observed_block_interval_ms = 372`, `avg_block_time_ms = 380`, and `validator_count = 4` |
| Public WebSocket | `wss://testnet-rpc.lichen.network/ws` passed the public readiness WebSocket check |
| Public faucet | `https://faucet.lichen.network/health` returned `OK`; `/faucet/status` returned faucet address and balance |
| Public DEX/oracle smoke | `/api/v1/oracle/prices` returned fresh wrapped-asset feeds with wSOL/wBTC slot `6873181`; `/api/v1/pairs/2/candles?interval=60&limit=4` returned 20 candle rows |
| Public exchange simulation | `EXCHANGE_SIM_FUNDING_MODE=faucet RPC_URL=https://testnet-rpc.lichen.network EXCHANGE_SIM_FAUCET_URL=https://faucet.lichen.network ./.venv/bin/python scripts/qa/exchange_simulation.py` passed |
| Public exchange report | `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json` recorded customer funding, deposit, finalized transaction lookup, account history, finality buffers, sweep, withdrawal, CLI smoke, and reconciliation |

## Phase 6 Native Scope Evidence

Native LICN scope separation was reviewed on 2026-06-29.

| Check | Result |
| --- | --- |
| Native guide scope | `docs/guides/EXCHANGE_INTEGRATION.md` states that native LICN deposits and withdrawals do not require DEX, wrapped assets, bridge custody, or oracle contracts |
| Native deposit flow | Deposit and withdrawal sections use native Base58 accounts, canonical JSON-RPC, raw spores, `sendTransaction`, `getTransaction`, and account history; they do not depend on custody REST, DEX routes, wrapped-asset reserves, or oracle feeds |
| Custody source map | `custody/src/bootstrap_support/router.rs` exposes `/health`, `/status`, `/deposits`, `/withdrawals`, reserves, webhooks, and event streams as a separate custody/bridge service surface |
| DEX source map | `rpc/src/dex.rs` exposes DEX REST routes for pairs, orderbooks, routes, pools, margin, stats, and oracle prices; DEX writes are not part of the native LICN deposit cookbook |
| Oracle source map | `contracts/lichenoracle/src/lib.rs`, `core/src/processor/governance_oracle.rs`, and `rpc/src/dex.rs` are mapped as optional ecosystem/oracle context, not native deposit prerequisites |
| Operations pack status policy | `docs/deployment/EXCHANGE_OPERATIONS_PACK.md` now says optional custody/bridge, DEX, and oracle surfaces need separate status coverage if included in a listing package |
| Live-liquidity claim control | The exchange integration guide does not publish DEX pair counts, route liquidity, reserve levels, or oracle freshness as listing claims; those remain source-mapped context only until separately verified |

## Phase 4 Exchange Simulation Evidence

Local exchange simulation ran on 2026-06-29 against a clean local `testnet`
three-validator stack.

| Check | Result |
| --- | --- |
| Start command | `scripts/start-local-stack.sh testnet` |
| Startup result | Validators on RPC ports `8899`, `8901`, and `8903` reached slot `81` with hash `3fed0c581669bdb109b3e87cf483e0921fa1ee7a2fa8b7246610454be74437b2`; custody and faucet were healthy |
| Simulation command | `RPC_URL=http://127.0.0.1:8899 ./.venv/bin/python scripts/qa/exchange_simulation.py` |
| Simulation report | `tests/artifacts/exchange-simulation-report.json` |
| Simulation slots | Started at slot `122`; finished at slot `169` |
| Deposit flow | Customer `5ehYMEk1YzK7WtpZ1gSk4ME1eYhGMMt5xnEV3kvq4k3` deposited `200000000` spores to deposit wallet `7YKDTkwQWmDx9auTwhAJMVEkBdmFPeeE485dgM5fHxy` |
| Deposit transaction | Signature `c99c0b7f1b984cf48773080fbdc72c834431625eae8e2c340ec3d435498c4bd0`, slot `125`; detected through account history and credited once |
| Sweep flow | Deposit wallet swept `190000000` spores to hot wallet `5AT42rMfm3NacE5QLYcCdsFEGhTJh9WGhTdaCucZHBg` |
| Sweep transaction | Signature `8e35a2ccf274ea2b62ca5d01d313f146e2537f520de6ffb49fea805a1d5ad656`, slot `134` |
| Withdrawal flow | Hot wallet withdrew `50000000` spores to destination `83zQK98vW9ETX3HEcX65dvxaV7RQGm1WtXZ91agWPZT` |
| Withdrawal transaction | Signature `ac4a0dd6d6bb2a16c007cfc526dd9b2c9b9a214cf362509ecd33b67f1a889fa1`, slot `136`; high-value plus-32 buffer reached before success |
| CLI smoke | CLI `balance`, `transfer`, `account history`, and `tx` lookup passed; CLI transfer signature `36079b78b4a054f88b1503e990661388dce55c5a5762ef0ee99f2cba9356efce`, slot `169`, credited cold wallet `10000000` spores |
| Reconciliation | Internal customer balance after withdrawal: `150000000` spores; final withdrawal destination balance: `50000000` spores; final cold balance after CLI transfer: `10000000` spores; deposit/hot/destination account history counts were `2`, `2`, and `1` |
| Stop command | `scripts/stop-local-stack.sh testnet` |
| Cleanup result | Status showed all validators, custody, and faucet down; process scan found no validator/sidecar processes; generated local credentials, signed manifest, state dirs, replay staging, and proposal staging dirs were removed |

## Phase 1 Output

Phase 1 created the external-package skeleton and the final testnet-only package
now publishes those artifacts under `exchange-testnet-v0.5.221`.

| Artifact | Status | Notes |
| --- | --- | --- |
| GitHub exchange integration guide | Published for testnet package | `docs/guides/EXCHANGE_INTEGRATION.md`; source-backed facts only, mainnet deferred explicitly |
| GitHub chain metadata sheet | Published for testnet package; status page active | `docs/guides/EXCHANGE_CHAIN_METADATA.md`; live URLs, logo, approved incident contacts, fee runtime value, rollback release metadata, package release metadata, and active `https://exchanges.lichen.network` status URL are recorded |
| GitHub operations pack | Published for testnet package; status page active | `docs/deployment/EXCHANGE_OPERATIONS_PACK.md`; incident aliases, exchange status portal policy, rollback/archive/history procedures, active exchange status URL, and final package release URLs are recorded |
| Developer portal exchange page | Published for testnet package | `developers/exchange-integration.html`; reviewer-facing content is inline on the portal, with release-tagged GitHub docs retained as source and audit links |
| Docs hub links | Created | `docs/README.md` links the exchange guide, metadata sheet, and operations pack |

## Phase 1 Blockers

| ID | Blocker | Impact | Required next step |
| --- | --- | --- | --- |
| B1-01 | Exchange guide was a draft and not externally approved | Resolved for testnet-only package | Published under `exchange-testnet-v0.5.221`; mainnet still deferred |
| B1-02 | Developer portal page linked out instead of carrying the exchange package | Resolved for testnet-only package | Portal now carries the exchange metadata, deposit/withdrawal cookbooks, finality/archive policy, operations contacts, validation gates, mainnet handoff, and release-tagged source links; readiness requires inline cookbook snippets |
| B1-04 | Operations contacts/status page were not approved | Resolved on 2026-07-05 | Contact aliases remain approved; `https://exchanges.lichen.network` is active, exchange-safe, non-admin, and passed default public readiness |

## Current Verification Pass

Latest local, deployment, and public verification ran on 2026-07-05 after the
exchange status-page activation. Local docs, portal assets, readiness unit
checks, exchange status page deploy, Cloudflare custom domain, same-origin
status RPC proxy, and default public readiness are green.
Mainnet is deliberately deferred until mainnet launch.

| Command | Result |
| --- | --- |
| `python3 -m py_compile scripts/qa/exchange_public_readiness.py scripts/qa/test_exchange_public_readiness.py scripts/qa/exchange_simulation.py` | Passed |
| `python3 scripts/qa/test_exchange_public_readiness.py` | Passed: `14` tests |
| `node scripts/qa/test_frontend_asset_integrity.js` | Passed: `376 passed, 0 failed` |
| `cargo test -p lichen-validator pre_consensus` | Passed: `2` tests |
| `cargo test -p lichen-validator live_bft_pauses_when_tip_or_observation_runs_ahead` | Passed: `1` test |
| `cargo test -p lobstercove-lichen-core account::tests::test_exchange_address_validation_vectors` | Passed |
| `cargo test -p lobstercove-lichen-core state::tests::exchange_archive_history_survives_hot_to_cold_migration_and_reopen` | Passed |
| `cargo test -p lichen-rpc test_native_archive_history_rpc_survives_cold_migration_and_reopen` | Passed |
| `cargo check --manifest-path sdk/rust/Cargo.toml` | Passed |
| `npm run build` in `sdk/js` | Passed |
| `./.venv/bin/python -m pytest sdk/python/test_connection_cleanup.py -q` | Passed: `4 passed` |
| `git diff --check` | Passed |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-domain-20260705-post-status-logic-green.json` | Passed every gate against `https://exchanges.lichen.network`; public testnet RPC/WS/explorer/developer/status/release checks are green |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-url https://03e74d4f.lichen-network-exchanges.pages.dev --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-preview-green.json` | Historical 2026-07-02 result: passed every gate against the deployed Pages preview |
| `python3 scripts/qa/exchange_public_readiness.py` before recovery | Failed closed as expected on public blockers: stale testnet RPC, undeployed developer exchange page content, status approval, final exchange package tag, and full-scope mainnet readiness not yet available |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet` after recovery, before page deploy | Testnet public RPC/WS checks passed; gate still failed closed on stale public developer page scope marker, status approval, and final exchange package tag |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --report /tmp/lichen-exchange-public-readiness-scope-testnet-rerun.json` before page deploy | Historical result superseded by the 2026-07-02 status-page correction; at the time it failed closed on expected package blockers, but the old internal candidate is no longer allowed as an exchange status page |
| `bash scripts/deploy-cloudflare-pages.sh developers` | Passed; frontend asset integrity reran with `359 passed, 0 failed`; Cloudflare Pages deployed `developers/` to `lichen-network-developers` |
| Public developer portal verification loop | Passed: `https://developers.lichen.network/exchange-integration` returned HTTP `200`, 12,303-byte body, and contained `testnet-only`, `mainnet launch exchange handoff`, and `Exchange Operations Pack` |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --report /tmp/lichen-exchange-public-readiness-scope-testnet-post-deploy.json` | Historical result superseded by the 2026-07-02 status-page correction; the current gate must verify `https://exchanges.lichen.network` instead of the old internal candidate |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --report /tmp/lichen-exchange-public-readiness-scope-testnet-final.json` | Historical result superseded by the 2026-07-02 status-page correction; the current gate must verify `https://exchanges.lichen.network` instead of the old internal candidate |
| Developer portal update after status approval | Superseded by the 2026-07-04 redeploy: the live portal removes the admin monitoring URL, removes the old planned wording, and records `https://exchanges.lichen.network` as the exchange status/operations page. |
| `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --report tests/artifacts/exchange-public-readiness-v0.5.221-status-approved.json` | Historical result superseded by the 2026-07-05 status-page activation; the current gate verifies `https://exchanges.lichen.network` and requires `--release-tag-selected` |
| Incident/contact alias approval | Operator approved `security@lichen.network`, `exchange-ops@lichen.network`, and `business@lichen.network` on 2026-07-01; operations pack records critical acknowledgement, active update, maintenance notice, emergency exception, authenticated outbound, and backup-path policy |
| GitHub CI for commit `e2bdd7aa` | Passed: `Test`, `Cargo Deny`, `Cargo Audit`, `Clippy`, `Format`, `Integration Tests`, `Docker Build`, `WASM Contract Builds`, prediction market, wallet extension, dependency health, and SBOM jobs |
| OpenSSF Scorecard | Passed |
| GitHub CI `Test` job for `e2bdd7aa` | Passed |
| `cargo deny check --config deny.toml advisories licenses sources` | Passed |
| `cargo audit -q -D warnings` | Passed |
| `cargo test -p lichen-faucet -- --nocapture` | Passed: `13 passed; 0 failed` |
| `cargo test -p lichen-validator --bin lichen-validator -- --nocapture` | Passed: `334 passed; 0 failed` |
| `bash tests/local-multi-validator-test.sh 4` | Passed; joiner restart preserved keypair, avoided genesis reimport, caught up with drift `0`, and cleaned up |
| Local faucet-backed exchange simulation | Passed against clean local testnet stack |
| Release workflow for tag `v0.5.221` | Passed; release artifacts published, checksummed, signed, downloaded, and verified |
| Live runbook verify-only for `v0.5.221` | Passed: `RELEASE VERIFY COMPLETE` |
| Public testnet exchange simulation | Passed; report written to `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json` |
| Public RPC/faucet final sample | Passed: `getHealth.status = ok`, slot `6871959`, `block_age_secs = 1`; faucet `/health = OK` |
| Final package tag | `exchange-testnet-v0.5.221` selected for the testnet-only exchange package |
| Final package release | `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221` with `lichen-exchange-testnet-v0.5.221.tar.gz` and `SHA256SUMS` |

## Next Ordered Work

1. Keep `https://exchanges.lichen.network` in the default readiness gate for every exchange package refresh and after any RPC, portal, or Cloudflare deployment.
2. At mainnet launch, run the mainnet launch exchange handoff gate and `scripts/qa/exchange_public_readiness.py --scope full` before publishing any mainnet exchange package.
3. For future Lichen token listings, complete the dedicated token exchange integration tracker before claiming token-listing readiness.
