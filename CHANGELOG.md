# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.7] - 2026-04-23

### Fixed
- Warp checkpoint verification now accepts finalized checkpoint contents authenticated by a signed committed header while corroborating checkpoint roots by verified validator identity instead of peer socket address.
- Warp snapshot serving now includes validator and stake singleton state, avoids repeated full-column scans while paginating snapshot chunks, and falls back to the newest valid checkpoint when the latest checkpoint metadata is bad.
- Catch-up sync no longer overlaps in-flight ranges prematurely and completes batches only once the requested target slot is actually reached.
- Monitoring incident controls no longer present unsupported production RPC kill switches, and the LichenSwap stats RPC method name now matches the backend.
- RPC validator liveness status is now computed consistently across cluster and validator endpoints.

## [0.5.6] - 2026-04-10

### Added
- `lichen identity export` CLI command: decrypt and display validator/wallet keypair info. Supports `--reveal-seed` for private key export and `--output json` for agent-friendly output.
- `deploy/setup.sh` now auto-generates `LICHEN_KEYPAIR_PASSWORD` if not previously set, eliminating a manual step that could be missed during deployment.

### Fixed
- Block timestamp drift: added `wall_clock_safe_delay()` to prevent block timestamps from racing ahead of wall clock time during fast BFT rounds. Previously, second-precision timestamps with 400ms slot time caused ~0.6s drift per block, triggering the 120s future-block rejection threshold after ~200 blocks.
- Deployment runbook: Step 5 (signed metadata manifest) is now marked mandatory with verification commands. Missing manifest was the root cause of DEX "Missing contract addresses" errors on deployed frontends.

### Changed
- Deployment runbook Step 4: documents that `LICHEN_KEYPAIR_PASSWORD` is auto-generated and shows how to use `lichen identity export` to access validator keys.
- Deployment runbook Step 6: joining validators now explicitly copy the signed metadata manifest from the genesis VPS.

## [0.5.5] - 2026-04-07

### Changed
- Removed validator bootstrap flag and environment override paths in favor of seed-file-only peer discovery.
- Updated local test harnesses, deployment setup, and operator docs to stage and consume `seeds.json` directly.
- Changed release archives to ship `zk-prove` with validator bundles and dropped faucet/custody binaries from the public agent install path.

## [0.5.4] - 2026-04-06

### Changed
- Bumped Rust crate versions for the testnet recovery and redeploy cycle.
- Aligned the testnet custody ingress hostname with `custody-testnet.lichen.network`.

## [0.4.37] - 2026-03-29

### Changed
- SDK versions bumped to 1.0.0 (JavaScript, Python, Rust contract SDK)
- Python SDK migrated from `setup.py` to `pyproject.toml` (PEP 517/518)
- CLI `--template` now validates against known categories
- CLI `init` command deprecated in favor of `identity new`
- CLI help text no longer hardcodes fee amounts; directs users to `lichen fees`
- Deprecated staking methods (`stakeToMossStake`, `unstakeFromMossStake`, `claimUnstakedTokens`) now return error code `-32000` (deprecated) instead of `-32601` (method not found)
- Solana compatibility layer returns descriptive error with supported method list for unsupported methods
- `getTransactionsByAddress` and `getTransactionHistory` consolidated to single handler (both names still work)
- `getAllSymbols` added as alias for `getAllSymbolRegistry`
- JS SDK `Connection` now supports configurable request timeout (default: 30s)
- Makefile `build-sdk` no longer suppresses TypeScript stderr
- **BREAKING**: `compute_tx_root` now uses a binary Merkle tree (domain-separated SHA-256) instead of flat concatenated hash. Blocks produced by v0.4.37+ are not compatible with older validators.

### Added
- `CHANGELOG.md` — this file
- `SECURITY.md` — responsible disclosure policy
- Binary Merkle tree for transaction root: `merkle_tx_root_from_hashes`, `merkle_tx_proof`, `verify_merkle_tx_proof` (Plan D — PR-02/BS-01)
- `getTransactionProof` RPC method — returns Merkle inclusion proof for any transaction
- JS SDK `getTransactionProof()` and static `verifyTransactionProof()` methods with `ProofStep` and `TransactionProof` types
- `lichen contract generate-client` CLI command — generates typed TypeScript or Python client from contract ABI (Plan E — DX-01)
- `allowance()` export added to lichencoin contract (Plan B — BS-03)
- Dual dispatch pattern documented in developer portal contract reference (Plan C — BS-04)

### Fixed
- JS SDK `package.json` repository URL corrected to `lobstercove/lichen`

### Removed
- MoltChain egg-info artifacts removed from source tree
- Python virtual environment removed from source tree
- JS SDK `dist/` removed from source tracking

## [0.4.36] - 2026-03-28

### Added
- Production readiness audit (`docs/PRODUCTION_READINESS_AUDIT_MARCH_2026.md`)
- Security audit (`docs/SECURITY_AUDIT_MARCH_2026.md`)

## [0.4.35] - 2026-03-27

### Changed
- Clean-slate redeploy: all frontends, contracts, and genesis regenerated
- BFT consensus stabilized across 3 VPS validators (US/EU/SEA)

## [0.4.34] - 2026-03-26

### Fixed
- Validator auto-update and built-in supervisor
- Genesis `initial_validators` BFT fix

## [0.4.33] - 2026-03-25

### Added
- Cross-margin DEX design (`docs/CROSS_MARGIN_DESIGN.md`)
- Prediction market contracts and RPC endpoints

### Changed
- WASM contracts rebuilt for deterministic genesis
