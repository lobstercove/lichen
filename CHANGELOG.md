# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
