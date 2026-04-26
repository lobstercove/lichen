# 2026-04-26 - Mainnet Genesis Price Hardening

## Objective

Investigate why fresh DEX charts opened at a low SOL price after reset, then harden genesis price seeding so mainnet cannot silently use stale compiled market defaults.

## Findings

- The visible first-candle jump was a real genesis bootstrap artifact, not frontend cache.
- `lichen-genesis` attempted a Binance REST fetch once. If that fetch failed, it used `GenesisPrices::default()`, including `wSOL=$81.84`.
- Genesis seeded that value into oracle, DEX analytics, margin, and initial TradingView candles. The live validator oracle then updated the same first candle to the current feed price, creating a fake open/low.

## Changes

- `genesis/src/main.rs`
  - Added `--genesis-prices-file <path>` support.
  - Added complete `GENESIS_SOL_USD`/`GENESIS_ETH_USD`/`GENESIS_BNB_USD` environment override support.
  - Added live price fallback from Binance, then CoinGecko.
  - Mainnet now exits if no explicit/env/live price source succeeds.
  - Testnet/dev still fall back to compiled defaults for convenience.
  - Added price parsing/snapshot unit tests.
- `scripts/clean-slate-redeploy.sh`
  - No longer exports hardcoded fallback prices.
  - Writes a `genesis-prices.json` snapshot when Binance returns all required tickers and passes it into `lichen-genesis`.
- `scripts/generate-genesis.sh`
  - Passes through `--genesis-prices-file`.
- `lichen-start.sh` and `run-validator.sh`
  - Removed hardcoded fallback env exports from local prefetch helpers.
- `deploy/setup.sh`
  - Updated operator guidance to mention mainnet fail-closed price behavior and the snapshot file.

## Validation

- `cargo fmt --all --check`
- `cargo test -p lichen-genesis --bin lichen-genesis`
- `cargo clippy -p lichen-genesis --bin lichen-genesis -- -D warnings`
- `cargo check --workspace`
- `bash -n scripts/clean-slate-redeploy.sh scripts/generate-genesis.sh lichen-start.sh run-validator.sh deploy/setup.sh`

