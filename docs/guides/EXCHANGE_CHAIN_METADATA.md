# Lichen Exchange Chain Metadata

**Status:** Draft, source-mapped but not externally approved
**Created:** 2026-06-29
**Integration guide:** [EXCHANGE_INTEGRATION.md](EXCHANGE_INTEGRATION.md)
**Address vectors:** [EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](EXCHANGE_ADDRESS_VALIDATION_VECTORS.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Rollback anchor:** `v0.5.221`, per operator update on 2026-07-01

This sheet is the canonical exchange metadata work area. It must not be sent as a
final listing sheet until every `Blocked` or `Needs verification` row below is
resolved with evidence.

Current package scope: testnet-only integration testing until mainnet launch.
Mainnet rows below are launch placeholders and must not be used in an external
listing sheet until the mainnet launch runbook closes its exchange handoff gate.

## Listing Metadata

| Field | Current value | Status | Source |
| --- | --- | --- | --- |
| Chain name | Lichen | Source mapped | Product docs and package metadata |
| Native asset | LICN | Source mapped | Foundation/tokenomics docs and RPC examples |
| Ticker | `LICN` | Source mapped | Foundation/tokenomics docs and SDK metadata |
| Decimals | `9` | Source mapped | `core/src/account.rs` |
| Base unit | `spore` | Source mapped | `core/src/account.rs` |
| Unit conversion | `1 LICN = 1,000,000,000 spores` | Source mapped | `core/src/account.rs` |
| Fee unit | Native LICN spores | Source mapped | `core/src/processor/fees.rs`, `core/src/genesis.rs`, `rpc/src/lib.rs` |
| Default base fee | `1,000,000` spores | Public testnet runtime `getFeeConfig` verified after signed `v0.5.221` recovery rollout on 2026-07-01 | `core/src/genesis.rs`, runtime `getFeeConfig`, tracker Phase 5 metadata evidence |
| Native mainnet chain ID | `lichen-mainnet-1` | Source mapped | `seeds.json`, `core/src/network.rs` |
| Native testnet chain ID | `lichen-testnet-1` | Source mapped | `seeds.json`, `core/src/network.rs` |
| EVM compatibility chain ID | Query `/evm` `eth_chainId` at runtime; live testnet returned `0xca3f1595a6c25e9f` on 2026-06-30. `8001` is a core compatibility/default constant, not the native LICN listing chain ID. | Source mapped and public testnet verified | `core/src/evm.rs`, `rpc/src/lib.rs`, public testnet `eth_chainId` |
| Native address format | Base58-encoded 32-byte account ID | Source mapped and tested | `core/src/account.rs` |
| Address validation | Decode Base58 and require exactly 32 decoded bytes | Source mapped and tested | `core/src/account.rs`, `EXCHANGE_ADDRESS_VALIDATION_VECTORS.md` |
| Address regex | `^[1-9A-HJ-NP-Za-km-z]{32,44}$` as prefilter only; decoded length must be exactly 32 bytes | Source mapped and tested | `core/src/account.rs`, `EXCHANGE_ADDRESS_VALIDATION_VECTORS.md` |
| Memo/tag requirement | None for native LICN base transfer flow | Locally validated | Native transfer/account model, local exchange simulation |
| Mainnet RPC URL | `https://rpc.lichen.network` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `seeds.json`, `core/src/network.rs`, `developers/shared-config.js`, mainnet launch runbook |
| Mainnet WebSocket URL | `wss://rpc.lichen.network/ws` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `developers/shared-config.js`, mainnet launch runbook |
| Testnet RPC URL | `https://testnet-rpc.lichen.network` | Healthy after signed `v0.5.221` recovery rollout on 2026-07-01; sustained public cadence sampled `370.0ms/block`, public `getMetrics.observed_block_interval_ms = 372`, and `avg_block_time_ms = 380` | `seeds.json`, `core/src/network.rs`, `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Testnet WebSocket URL | `wss://testnet-rpc.lichen.network/ws` | Public readiness WebSocket upgrade passed after signed `v0.5.221` recovery rollout on 2026-07-01; live slot notifications advanced `6871609` -> `6871611` | `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Explorer URL | `https://explorer.lichen.network` | Route templates verified on 2026-06-29 | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js`, tracker Phase 5 metadata evidence |
| Logo URL | `https://lichen.network/Lichen_Logo_256.png` | Public asset verified on 2026-06-29: PNG, 256x256, SHA-256 matches repo asset | `website/Lichen_Logo_256.png`, tracker Phase 5 metadata evidence |
| Status page | `https://monitoring.lichen.network` | Operator-approved exchange status page for the current testnet-only package on 2026-07-01; private operator surface | Operations pack policy |
| Release verification | GitHub release `v0.5.221` has `SHA256SUMS` plus `SHA256SUMS.sig`; PQ signature verified locally | Current signed rollback anchor and testnet recovery release verified; final external docs package publication still required | `.github/workflows/release.yml`, `scripts/sign-release.sh`, `scripts/verify-release-checksums.mjs`, GitHub release API |
| Release signer | `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` | Source mapped | `deploy/release-trust-anchor.json` |
| Current rollback anchor | `v0.5.221` | Operator anchored | Operator update on 2026-07-01 |

## Native Address Validation

External exchange docs must publish validation as code, not only as a regex.

Required rule:

```text
valid_native_address(address):
    bytes = base58_decode(address)
    return len(bytes) == 32
```

Rejected cases:

- Empty string.
- Non-Base58 characters.
- Valid Base58 string decoding to fewer than 32 bytes.
- Valid Base58 string decoding to more than 32 bytes.
- EVM `0x...` address submitted where a native LICN address is required.

Test vectors are published in
[EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](EXCHANGE_ADDRESS_VALIDATION_VECTORS.md).

## Native Versus EVM Compatibility

Listings must not conflate the native Lichen chain ID with the EVM compatibility
surface.

Native integration:

- Chain IDs are strings such as `lichen-mainnet-1` and `lichen-testnet-1`.
- Native addresses are Base58-encoded 32-byte account IDs.
- Native LICN transfers are signed with native transaction envelopes and submitted
  through `sendTransaction`.

EVM compatibility:

- `/evm` `eth_chainId` returns `RpcState.evm_chain_id`.
- `rpc/src/lib.rs` derives `RpcState.evm_chain_id` by hashing the configured
  native chain ID. On public testnet, `getNetworkInfo.chain_id` returned
  `lichen-testnet-1` and `/evm` `eth_chainId` returned
  `0xca3f1595a6c25e9f` on 2026-06-30.
- `core/src/evm.rs` declares `LICHEN_CHAIN_ID = 8001`, but that is a core
  compatibility/default constant. It is not the native LICN listing chain ID and
  must not be used for native deposit/withdrawal signing.
- EVM-format addresses are derived mappings from native pubkeys, not the native
  LICN deposit address format.

Exchange native LICN listings should publish the native string chain ID and
Base58 account format. If an exchange separately tests EVM compatibility, it
must query `eth_chainId` from the target network at integration time instead of
hard-coding `8001`.

## Explorer URL Patterns

Base explorer URL:

```text
https://explorer.lichen.network
```

Exchange-facing public route templates verified on 2026-06-29:

- Account/address page: `https://explorer.lichen.network/address?address={base58_address}`
- Transaction page: `https://explorer.lichen.network/transaction?sig={transaction_signature}`
- Block page by slot: `https://explorer.lichen.network/block?slot={slot}`

Source/static implementation details:

- `explorer/js/address.js` reads `address` and legacy `addr` query
  parameters.
- `explorer/js/transaction.js` accepts `sig`, `tx`, `hash`, and `signature`;
  exchange-facing docs should use `sig`.
- `explorer/js/block.js` accepts `slot` and legacy `block` query parameters;
  exchange-facing docs should use `slot`.
- Static source files use `.html` routes. The deployed site redirects
  `.html` route requests to extensionless public routes with `308`, then returns
  `200`.

The hosted explorer root, account, transaction, and block routes returned
`200` in external HTTP checks on 2026-06-29. The production frontend config
defines hosted RPC/WS connectivity to `https://testnet-rpc.lichen.network` and
`wss://testnet-rpc.lichen.network/ws` by default; route verification does not
mean the public RPC was healthy during the same check.

## Release Verification Metadata

Source-backed release artifacts:

- `SHA256SUMS`
- `SHA256SUMS.sig`
- Trust anchor: `deploy/release-trust-anchor.json`
- Signing helper: `scripts/sign-release.sh`
- Verification helper: `scripts/verify-release-checksums.mjs`
- Release workflow: `.github/workflows/release.yml`

Verified rollback-anchor release metadata on 2026-07-01:

- Release page: `https://github.com/lobstercove/lichen/releases/tag/v0.5.221`
- GitHub release is published, not a draft, and not a prerelease.
- Assets listed by the API: Linux, macOS, and Windows validator archives,
  `SHA256SUMS`, and `SHA256SUMS.sig`.
- `SHA256SUMS` and `SHA256SUMS.sig` both downloaded successfully from the
  `v0.5.221` release.
- `scripts/verify-release-checksums.mjs` against the downloaded `v0.5.221`
  release artifacts
  verified the PQ signature against signer
  `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Current signed testnet recovery release metadata on 2026-07-01:

- Release page: `https://github.com/lobstercove/lichen/releases/tag/v0.5.221`
- GitHub release is published, not a draft, and not a prerelease.
- Linux release archives include `lichen-validator`, `lichen-custody`, and
  `lichen-faucet`.
- `SHA256SUMS` and `SHA256SUMS.sig` downloaded from the release.
- `scripts/verify-release-checksums.mjs` verified all archive hashes and the PQ
  signature against signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Pending blocker: attach the final external docs/package artifacts before
publishing exchange-facing release links outside the repository.

## Remaining Final Metadata Blockers

| ID | Blocker | Required evidence |
| --- | --- | --- |
| M-07 | Final external package publication not approved | Operator-approved publication evidence for the exchange package, not only validator release artifacts |

## Deferred Mainnet Launch Items

These are not blockers for the current testnet-only package, but they are
mandatory before mainnet is included in an external exchange package.

| ID | Deferred item | Required evidence |
| --- | --- | --- |
| MM-01 | Mainnet public RPC/WS readiness | `https://rpc.lichen.network` and `wss://rpc.lichen.network/ws` pass health, fee, finalized-slot, latest-block, WebSocket subscription, archive/history, and exchange simulation checks after mainnet launch |
| MM-02 | Mainnet metadata refresh | Mainnet `getNetworkInfo`, `/evm` `eth_chainId`, fee config, explorer routes, release tag, status page, and incident contacts recorded in this sheet |
| MM-03 | Mainnet exchange handoff | Mainnet launch runbook exchange handoff gate closed and public readiness gate rerun with `--scope full` |

## Resolved Metadata Checks

| ID | Check | Evidence |
| --- | --- | --- |
| M-06 | Status page and incident contacts approved | `https://monitoring.lichen.network` operator-approved on 2026-07-01 for the current testnet-only package; `security@lichen.network`, `exchange-ops@lichen.network`, and `business@lichen.network` recorded in the operations pack |
| M-04 | Explorer route templates | Source route inspection plus hosted `200` checks for root, account, transaction, and block pages on 2026-06-29 |
| M-05 | Logo URL cache verification | `https://lichen.network/Lichen_Logo_256.png` returned `200`, `image/png`, 45,415 bytes; downloaded file is PNG 256x256 and SHA-256 `bfa0986bc4bde64c3c7ce590782beba78980985f301fbd0fbd4a39dc045ca876`, matching `website/Lichen_Logo_256.png` |
| M-07 rollback subset | `v0.5.221` rollback-anchor release signatures | GitHub release is published, checksum/signature assets downloaded, and `scripts/verify-release-checksums.mjs` verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` |
| M-02 | Runtime fee value refreshed on public testnet | `getFeeConfig` returned `base_fee_spores = 1000000`, contract/NFT fee fields, and `40/30/10/10/10` fee split after signed `v0.5.221` recovery rollout |
| M-09 | Testnet RPC/WS readiness after final rollout | Public `getHealth` returned `status = ok`; sustained public cadence sampled `370.0ms/block`; public `getMetrics` returned `observed_block_interval_ms = 372` and `avg_block_time_ms = 380`; WebSocket readiness and live slot notifications passed |
| M-10 | Current signed testnet recovery release signatures | `v0.5.221` release checksum and detached PQ signature were verified against `deploy/release-trust-anchor.json`; live runbook verify-only completed `RELEASE VERIFY COMPLETE` |
| M-08 | EVM chain ID wording reconciled for native listings | Native exchange integrations use string chain IDs from `getNetworkInfo`; EVM compatibility uses runtime `/evm` `eth_chainId`; live testnet returned `0xca3f1595a6c25e9f`; `8001` is documented as a core compatibility/default constant, not the native listing chain ID |
