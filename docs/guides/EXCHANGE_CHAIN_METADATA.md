# Lichen Exchange Chain Metadata

**Status:** Draft, source-mapped but not externally approved
**Created:** 2026-06-29
**Integration guide:** [EXCHANGE_INTEGRATION.md](EXCHANGE_INTEGRATION.md)
**Address vectors:** [EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](EXCHANGE_ADDRESS_VALIDATION_VECTORS.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Rollback anchor:** `v0.5.215`, per operator note on 2026-06-29

This sheet is the canonical exchange metadata work area. It must not be sent as a
final listing sheet until every `Blocked` or `Needs verification` row below is
resolved with evidence.

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
| Default base fee | `1,000,000` spores at genesis defaults | Runtime public verification blocked by stale public RPC | `core/src/genesis.rs`, runtime `getFeeConfig`, tracker Phase 5 metadata evidence |
| Native mainnet chain ID | `lichen-mainnet-1` | Source mapped | `seeds.json`, `core/src/network.rs` |
| Native testnet chain ID | `lichen-testnet-1` | Source mapped | `seeds.json`, `core/src/network.rs` |
| EVM compatibility ID | `8001` in core EVM compatibility code; runtime RPC derives an EVM chain ID from native chain ID | Needs final wording | `core/src/evm.rs`, `rpc/src/lib.rs` |
| Native address format | Base58-encoded 32-byte account ID | Source mapped and tested | `core/src/account.rs` |
| Address validation | Decode Base58 and require exactly 32 decoded bytes | Source mapped and tested | `core/src/account.rs`, `EXCHANGE_ADDRESS_VALIDATION_VECTORS.md` |
| Address regex | `^[1-9A-HJ-NP-Za-km-z]{32,44}$` as prefilter only; decoded length must be exactly 32 bytes | Source mapped and tested | `core/src/account.rs`, `EXCHANGE_ADDRESS_VALIDATION_VECTORS.md` |
| Memo/tag requirement | None for native LICN base transfer flow | Locally validated | Native transfer/account model, local exchange simulation |
| Mainnet RPC URL | `https://rpc.lichen.network` | Live check failed: Cloudflare `525` on 2026-06-29 | `seeds.json`, `core/src/network.rs`, `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Mainnet WebSocket URL | `wss://rpc.lichen.network/ws` | Live check failed: Cloudflare `525` on 2026-06-29 | `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Testnet RPC URL | `https://testnet-rpc.lichen.network` | Reachable but not exchange-ready on 2026-06-30: `health.status = behind`, `reason = stale_tip`, slot `6715444`; recovery is pending signed `v0.5.216` rollout | `seeds.json`, `core/src/network.rs`, `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Testnet WebSocket URL | `wss://testnet-rpc.lichen.network/ws` | TLS/WebSocket upgrade passed; app readiness still blocked by stale public RPC | `developers/shared-config.js`, tracker Phase 5 metadata evidence |
| Explorer URL | `https://explorer.lichen.network` | Route templates verified on 2026-06-29 | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js`, tracker Phase 5 metadata evidence |
| Logo URL | `https://lichen.network/Lichen_Logo_256.png` | Public asset verified on 2026-06-29: PNG, 256x256, SHA-256 matches repo asset | `website/Lichen_Logo_256.png`, tracker Phase 5 metadata evidence |
| Status page | Candidate: `https://monitoring.lichen.network` | Public monitoring app reachable; not operator-approved as exchange status page | Operations pack decision required |
| Release verification | GitHub release `v0.5.215` has `SHA256SUMS` plus `SHA256SUMS.sig`; PQ signature verified locally | Rollback release verified; final exchange package tag still required | `.github/workflows/release.yml`, `scripts/sign-release.sh`, `scripts/verify-release-checksums.mjs`, GitHub release API |
| Release signer | `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` | Source mapped | `deploy/release-trust-anchor.json` |
| Current rollback anchor | `v0.5.215` | Operator anchored | Operator note on 2026-06-29 |

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

- `core/src/evm.rs` declares `LICHEN_CHAIN_ID = 8001`.
- `rpc/src/lib.rs` also derives an EVM chain ID from the configured native chain
  ID for runtime RPC state.
- EVM-format addresses are derived mappings from native pubkeys, not the native
  LICN deposit address format.

Final external wording must be settled after runtime RPC behavior and EVM docs are
checked together.

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

Verified rollback release metadata on 2026-06-29:

- Release page: `https://github.com/lobstercove/lichen/releases/tag/v0.5.215`
- GitHub API fields: `tag_name = v0.5.215`, `draft = false`,
  `prerelease = false`, `published_at = 2026-06-29T00:47:53Z`.
- Assets listed by the API: Linux, macOS, and Windows validator archives,
  `SHA256SUMS`, and `SHA256SUMS.sig`.
- `SHA256SUMS` and `SHA256SUMS.sig` both downloaded successfully from the
  `v0.5.215` release.
- `node scripts/verify-release-checksums.mjs /tmp/lichen-v05215-release`
  verified the PQ signature against signer
  `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Pending blocker: confirm the intended exchange-package release tag and attach
the final external docs/package artifacts before publishing exchange-facing
release links.

## Remaining Final Metadata Blockers

| ID | Blocker | Required evidence |
| --- | --- | --- |
| M-02 | Runtime fee value not publicly verified on the current live public target | `getFeeConfig` result from a healthy public target network after `v0.5.216` recovery rollout; 2026-06-30 public testnet is readiness-gated by stale tip |
| M-03 | Public RPC endpoints are not exchange-ready | Mainnet RPC/WS must stop returning Cloudflare `525`; testnet RPC must return healthy `health` and operational read/write methods |
| M-06 | Status page not finalized | Operator-approved URL and uptime policy |
| M-07 | Final exchange package release tag not selected | Signed release evidence for the external exchange package, not only rollback validator archives |
| M-08 | EVM chain ID wording not reconciled | Runtime RPC and EVM docs comparison |

## Resolved Metadata Checks

| ID | Check | Evidence |
| --- | --- | --- |
| M-04 | Explorer route templates | Source route inspection plus hosted `200` checks for root, account, transaction, and block pages on 2026-06-29 |
| M-05 | Logo URL cache verification | `https://lichen.network/Lichen_Logo_256.png` returned `200`, `image/png`, 45,415 bytes; downloaded file is PNG 256x256 and SHA-256 `bfa0986bc4bde64c3c7ce590782beba78980985f301fbd0fbd4a39dc045ca876`, matching `website/Lichen_Logo_256.png` |
| M-07 rollback subset | `v0.5.215` rollback release signatures | GitHub release is published, checksum/signature assets downloaded, and `scripts/verify-release-checksums.mjs` verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk` |
