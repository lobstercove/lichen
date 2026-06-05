# BTC / wBTC Wrapped Asset Rollout Plan

This plan adds Bitcoin as a first-class wrapped asset on Lichen:

- future genesis includes a deterministic `wbtc_token` contract and WBTC symbol registry entry;
- the live chain can add WBTC post-genesis without rewriting historical state;
- custody can issue Bitcoin deposit addresses, detect BTC deposits, sweep to treasury, mint wBTC, burn wBTC, and broadcast BTC withdrawals;
- DEX, wallet, extension, explorer-facing RPC, manifests, and deployment docs expose BTC consistently.

The current codebase does not already implement Bitcoin custody. It only has a BIP-44 coin-type entry for Bitcoin, and tests currently assert that `derive_deposit_address("bitcoin", "btc", ...)` is unsupported. BTC therefore requires a real route implementation, not a token-list-only change.

## Current Source Of Truth

The existing wrapped asset model is:

| Source asset | Source route | Lichen token | Contract | Lichen accounting |
| --- | --- | --- | --- | --- |
| SOL | Solana native | wSOL | `wsol_token` | 9 decimals |
| ETH | Ethereum native | wETH | `weth_token` | 9 decimals |
| BNB | BSC native | wBNB | `wbnb_token` | 9 decimals |
| Neo X GAS | Neo X native | wGAS | `wgas_token` | 9 decimals |
| Neo X NEO | Neo X ERC-20 | wNEO | `wneo_token` | 9 decimals, whole NEO lots |
| USDC / USDT | Solana/EVM tokens | lUSD | `lusd_token` | 9 decimals |

Custody converts every supported source-chain amount into Lichen 9-decimal base units before minting. BTC has 8 native decimals, so the bridge conversion should be:

```text
1 satoshi = 10 Lichen wBTC base units
1 BTC     = 1_000_000_000 Lichen wBTC base units
```

That preserves exact satoshi accounting while keeping WBTC compatible with the current token, wallet, DEX, burn, reserve, and restriction code paths. A true 8-decimal Lichen token would require changing the wrapped-credit and wrapped-withdrawal conversion model everywhere, and would diverge from the current production wrapped-token standard.

## BTC Design Decisions

| Area | Decision |
| --- | --- |
| Registry symbol | `WBTC` in symbol registry, displayed as `wBTC` |
| Contract crate | `contracts/wbtc_token` |
| Contract name | `Wrapped BTC` |
| Contract token decimals | 9 Lichen base units, exact satoshi conversion through custody |
| Source chain aliases | `bitcoin`, `btc` |
| Source asset | `btc` |
| Deposit address format | native SegWit bech32, `bc1...` on mainnet; `tb1...` for testnet/local test routes |
| Bitcoin derivation | deterministic custody derivation under Bitcoin coin type 0, using a BTC-specific derivation/address helper |
| Genesis price env | `GENESIS_BTC_USD` |
| Market data | Binance `BTCUSDT`, CoinGecko `bitcoin` |
| DEX pair IDs | `12 = wBTC/lUSD`, `13 = wBTC/LICN` |
| Live-chain activation | gate BTC oracle/DEX mirror writes on `WBTC` symbol registration, same additive pattern as WNEO/WGAS |

## Scope

### Lichen Contract And Genesis

Add `wbtc_token` by following the current wrapped token contract model:

- copy the wBNB/wETH style receipt-token contract;
- change storage prefixes to `wbtc_`;
- set metadata to `Wrapped BTC` / `wBTC`;
- set a BTC-appropriate epoch mint cap in 9-decimal units;
- add ABI and checked-in WASM artifact;
- include the crate in `scripts/build-all-contracts.sh`.

Update genesis:

- add `wbtc_token` to `GENESIS_CONTRACT_CATALOG`;
- initialize it with the operational token admin;
- mark it as an operational token contract;
- register `.lichen` name `wbtc`;
- include WBTC in contract identity achievements;
- add `wbtc_usd_8dec` to `GenesisPrices` with a serde default for old config compatibility;
- add BTC to genesis price file validation, env overrides, Binance fetch, CoinGecko fetch, and genesis price logging;
- seed `wBTC/lUSD` and `wBTC/LICN` pairs and AMM pools in future genesis;
- register router routes for the new pair and pool IDs.

### Live Testnet Upgrade Path

The live v0.5.93 testnet must not replay history differently. BTC activation is post-genesis and additive:

- deploy `wbtc_token` using the existing contract deployment flow;
- register `WBTC` in the symbol registry;
- refresh `deploy-manifest.json` and signed metadata from the live registry;
- only after `WBTC` exists, validators submit/mirror BTC oracle data and DEX price bands for pair IDs 12 and 13.

No historical backfill, no hard-coded block migration, and no old state rewrite.

### Oracle And DEX

Update validator oracle feeder and mirror logic:

- track BTC price in `SharedOraclePrices`;
- add `btcusdt@aggTrade` to default Binance WS;
- add `BTCUSDT` to default Binance REST;
- parse BTC prices from WS and REST;
- submit native oracle attestations for `wBTC` only after `WBTC` is registered;
- mirror `price_wBTC` into oracle compatibility storage only after registration;
- write DEX price bands for pair IDs 12 and 13 only after registration.

Update RPC:

- add `getWbtcStats`;
- include WBTC in DEX token symbol maps;
- include `wBTC` in `getOraclePrices` and `/api/v1/oracle/prices`;
- map pair ID 12 to `wBTC` USD and pair ID 13 to `wBTC/LICN`;
- include WBTC in oracle operational stats only when `WBTC` is registered, matching the Neo gating pattern.

### Custody

Add a real Bitcoin route, separate from Solana and EVM:

- config fields:
  - `CUSTODY_BTC_RPC_URL`;
  - `CUSTODY_BTC_NETWORK=mainnet|testnet|regtest`;
  - `CUSTODY_BTC_CONFIRMATIONS`;
  - `CUSTODY_TREASURY_BTC`;
  - `CUSTODY_WBTC_TOKEN_ADDR`;
- source route support for `bitcoin`/`btc`;
- deterministic BTC deposit address derivation;
- Bitcoin address validation for withdrawals:
  - mainnet accepts `bc1...`;
  - testnet/regtest accepts `tb1...` or `bcrt1...`;
- Bitcoin watcher:
  - query deposit address transactions/UTXOs from the configured Bitcoin RPC;
  - wait for configured confirmations;
  - record confirmed BTC amount in satoshis;
  - enqueue sweep jobs;
- Bitcoin sweep:
  - spend deposit UTXOs to the BTC treasury address;
  - subtract transaction fee from native BTC deposits only when required by UTXO construction;
  - store sweep txid and wait for confirmations;
- credit:
  - `btc` maps to `wBTC`;
  - `source_chain_decimals("bitcoin", "btc") = 8`;
  - credit conversion is exact: satoshis multiply by 10;
- withdrawal:
  - `wbtc` burns map to `btc` outbound asset;
  - `spores_to_chain_amount(..., "btc")` divides by 10 and rejects non-satoshi dust;
  - broadcast spends from BTC treasury to the user's BTC destination address;
  - settlement confirms through Bitcoin RPC.

If the chosen Bitcoin RPC provider does not expose wallet/descriptor signing APIs for custody-owned keys, implement local PSBT/raw transaction signing in custody instead of relying on provider wallet state. Provider wallet state would be operationally fragile and not equivalent to the existing deterministic custody model.

### Deployment And Ops

Update route profiles and verification:

- `deploy/custody-env.example`;
- `deploy/custody-env-mainnet.example`;
- `deploy/custody-route-profile.md`;
- `scripts/apply-custody-route-profile.sh`;
- `scripts/verify-custody-routes.sh`;
- `scripts/sync-custody-wrapped-contracts.sh`;
- local 3-validator environment setup if it starts custody mocks.

For testnet/local BTC tests, use regtest or a controlled Bitcoin Core test instance. Do not point production custody at an unauthenticated public Bitcoin RPC.

### Frontends

Update all user-facing surfaces:

- wallet asset registry: add wBTC with 9 decimals, BTC reserve label, BTC logo;
- wallet deposit cards: add Bitcoin route and BTC token picker;
- extension bridge service: add `bitcoin`/`btc` support;
- extension popup, full page, and dashboard bridge selectors: add Bitcoin route;
- DEX pair labels and asset metadata if hardcoded;
- shared achievement descriptions: include WBTC;
- frontend QA tests for token lists, DEX pairs, and bridge route support.

### Docs And QA

Update:

- `docs/defi/WRAPPED_ASSETS.md`;
- deployment env docs and route runbooks;
- developer docs that enumerate genesis contracts, symbols, trading pairs, wrapped tokens, or contract counts;
- QA expected contract catalog.

Run focused tests before broader validation:

```bash
cargo fmt
cargo check -p lichen-custody
cargo test -p lichen-custody
cargo test -p lichen-genesis
cargo test -p lichen-validator apply_oracle
cargo test -p lichen-rpc oracle_stats
node scripts/qa/test_frontend_asset_integrity.js
node scripts/qa/test_wallet_extension_audit.js
node scripts/qa/test_deployment_env_examples.js
node scripts/qa/test_neo_developer_docs.js
```

## Local Three-Validator Validation Gate

After code and WASM are updated:

1. Stop the local stack.
2. Rebuild release binaries.
3. Rebuild changed contract WASM, including `wbtc_token`.
4. Reset the local testnet state.
5. Start three local validators from fresh genesis.
6. Verify all three validators advance slots and agree on the tip.
7. Verify `WBTC` exists in `getAllSymbolRegistry`.
8. Verify `getWbtcStats` returns the WBTC contract state.
9. Verify DEX pair count includes `wBTC/lUSD` and `wBTC/LICN`.
10. Verify oracle APIs include `wBTC` after local WBTC registration.
11. Verify wallet and extension tests see BTC in the bridge route list.
12. If a local Bitcoin RPC/regtest is available, run a BTC deposit, sweep, credit, burn, withdrawal, and confirmation smoke test.

The local BTC custody smoke is repeatable with Bitcoin Core installed:

```bash
cargo build --release --bin lichen-custody --bin lichen --bin bridge_auth_payload --bin withdrawal_auth_payload --bin wrapped_burn
./scripts/smoke-btc-regtest-custody.sh
```

Expected result:

- the script starts Bitcoin Core regtest and BTC-enabled custody locally;
- `createBridgeDeposit` accepts the `bitcoin:btc` route and returns a `bcrt1...` deposit address;
- custody detects the mined BTC deposit, broadcasts a BTC sweep to treasury, confirms the sweep, mints wBTC, accepts a wBTC burn, broadcasts a BTC withdrawal, and confirms the withdrawal;
- the script writes `data/btc-regtest-smoke/result.json` with `"status": "passed"`.

Last local run on 2026-06-05 passed with one confirmed sweep, one confirmed credit, and one confirmed withdrawal.

The VPS deployment commands should only be prepared after this local gate passes.

## VPS Deployment Gate

Do not change the live three VPSes until the local three-validator gate passes. The VPS rollout should be:

1. Build release artifacts locally or in CI.
2. Deploy validator/RPC/custody binaries consistently to all three VPSes.
3. Deploy `wbtc_token` once on the live chain.
4. Register `WBTC`.
5. Refresh signed metadata and deploy frontends.
6. Sync custody wrapped-token pins on all custody hosts.
7. Apply BTC route profile on all custody hosts.
8. Verify `--require-wrapped` and BTC route checks.
9. Restart custody.
10. Run a dust-sized BTC deposit/withdrawal smoke test.

Concrete additive command skeleton, to run only after a clean release is present on every host:

```bash
# On the deployer machine, build the WBTC contract artifact included in the release.
./scripts/build-all-contracts.sh --tokens

# Deploy/register WBTC once against the live testnet RPC using the governed deployer key.
LICHEN_RPC_URL=https://testnet-rpc.lichen.network \
  ./target/release/lichen deploy contracts/wbtc_token/wbtc_token.wasm \
  --keypair /path/to/governed-deployer.json \
  --symbol WBTC \
  --name "Wrapped BTC" \
  --template wrapped \
  --decimals 9 \
  --metadata '{"description":"Wrapped Bitcoin (BTC) on Lichen - bridged 1:1 from the Bitcoin network.","mintable":"true","burnable":"true","logo_url":"https://s2.coinmarketcap.com/static/img/coins/128x128/1.png","icon_class":"fab fa-bitcoin","total_supply":"0"}'

# Confirm the live registry entry before touching custody/frontends.
LICHEN_RPC_URL=https://testnet-rpc.lichen.network \
  ./target/release/lichen symbol lookup WBTC --output json

# Refresh operator manifests from the live symbol registry.
LICHEN_RPC_URL=https://testnet-rpc.lichen.network \
  python3 scripts/update-manifest.py

# On every custody host, fetch wrapped-token addresses and enforce the BTC route config.
./scripts/sync-custody-wrapped-contracts.sh https://testnet-rpc.lichen.network /etc/lichen/custody.env
./scripts/apply-custody-route-profile.sh testnet /etc/lichen/custody.env
./scripts/verify-custody-routes.sh /etc/lichen/custody.env --require-wrapped
```

Required custody values before enabling the BTC route:

```bash
CUSTODY_BTC_RPC_URL=https://...
CUSTODY_BTC_RPC_USER=...
CUSTODY_BTC_RPC_PASSWORD=...
CUSTODY_BTC_NETWORK=mainnet
CUSTODY_BTC_CONFIRMATIONS=6
CUSTODY_BTC_FEE_RATE_SATS_VB=5
CUSTODY_TREASURY_BTC=bc1...
CUSTODY_WBTC_TOKEN_ADDR=<live WBTC program from the symbol registry>
```

If BTC custody is not yet ready but WBTC contract and DEX support are ready, leave `CUSTODY_BTC_RPC_URL` unset and do not expose the Bitcoin deposit card in production frontends. The contract can exist before the route is public, but the UI must not present an unusable bridge path.

## Non-Goals

- No MossStake changes.
- No historical migration or root-changing replay behavior.
- No compatibility shim that pretends BTC custody works without a real Bitcoin route.
- No VPS mutation before local three-validator validation.
