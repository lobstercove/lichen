# BTC / wBTC Wrapped Asset Rollout Plan

This plan adds Bitcoin as a first-class wrapped asset on Lichen:

- fresh local-testnet and mainnet genesis includes deterministic `wbtc_token`/`WBTC` plus DEX pair/pool IDs `12` and `13`;
- existing live chains can activate WBTC additively from registry/metadata without replaying history, deleting state, or rewriting historical roots;
- custody can issue Bitcoin deposit addresses, detect BTC deposits, sweep to treasury, mint wBTC, burn wBTC, and broadcast BTC withdrawals;
- DEX, wallet, extension, explorer-facing RPC, manifests, and deployment docs expose BTC consistently.

The current codebase includes the WBTC contract, registry/oracle/DEX wiring, and Bitcoin custody route support. Operators still must provide real Bitcoin RPC credentials and complete deposit, sweep, burn, and withdrawal smoke tests before exposing the BTC route in public wallet surfaces.

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
- seed `wBTC/lUSD` and `wBTC/LICN` pairs and AMM pools in every fresh genesis;
- register router routes for the new pair and pool IDs.

### Live Testnet Upgrade Path

Existing live testnets must not replay history differently. BTC activation on those chains is post-genesis and additive:

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
6. Verify all validators advance slots and agree on the tip.
7. Verify `WBTC` exists in `getAllSymbolRegistry`.
8. Verify WBTC contract storage contains initialized admin/minter/attester and supply keys.
9. Verify `getWbtcStats` returns the WBTC contract state.
10. Verify DEX pair count includes `wBTC/lUSD` and `wBTC/LICN`.
11. Verify oracle APIs include `wBTC` after local WBTC registration.
12. Verify wallet and extension tests see BTC in the bridge route list.
13. If a local Bitcoin RPC/regtest is available, run a BTC deposit, sweep, credit, burn, withdrawal, and confirmation smoke test.

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

Do not change the live four VPSes until the local validation gate passes. The VPS rollout should be:

1. Build release artifacts locally or in CI.
2. Deploy validator/RPC/custody binaries consistently to all four VPSes.
3. Deploy `wbtc_token` once on the live chain.
4. Register `WBTC`.
5. Initialize WBTC with the operational token admin.
6. Verify WBTC contract storage is non-empty and contains initialized control keys.
7. Refresh signed metadata and deploy frontends.
8. Sync custody wrapped-token pins on all custody hosts.
9. Apply BTC route profile on all custody hosts.
10. Verify `--require-wrapped` and BTC route checks.
11. Restart custody.
12. Run a dust-sized BTC deposit/withdrawal smoke test.

Release rollout safety checks are mandatory before any live restart:

- verify `SHA256SUMS.sig` against the pinned release signer and then verify the archive hash from `SHA256SUMS`;
- require `lichen-validator`, `lichen-genesis`, `lichen`, `zk-prove`, `lichen-custody`, and `lichen-faucet` inside the selected release archive before touching a VPS;
- install binaries with temp+rename and verify `/usr/local/bin/*` hashes immediately after install;
- after every service restart, verify `/proc/<pid>/exe` is not a deleted executable and its hash matches the signed release archive;
- stop the rollout on the first mismatch. Do not continue to the next validator until the mismatched host has matching file and running-process hashes.

Concrete additive command skeleton, to run only after a clean release is present on every host:

```bash
# On the deployer machine, build the WBTC contract artifact included in the release.
./scripts/build-all-contracts.sh --tokens

# Deploy/register WBTC once against the live testnet RPC using the governed deployer key.
LICHEN_RPC_URL=https://testnet-api.lichen.network \
  ./target/release/lichen deploy contracts/wbtc_token/wbtc_token.wasm \
  --keypair /path/to/governed-deployer.json \
  --symbol WBTC \
  --name "Wrapped BTC" \
  --template wrapped \
  --decimals 9 \
  --metadata '{"description":"Wrapped Bitcoin (BTC) on Lichen - bridged 1:1 from the Bitcoin network.","mintable":"true","burnable":"true","logo_url":"https://s2.coinmarketcap.com/static/img/coins/128x128/1.png","icon_class":"fab fa-bitcoin","total_supply":"0"}'

# Confirm the live registry entry before touching custody/frontends.
LICHEN_RPC_URL=https://testnet-api.lichen.network \
  ./target/release/lichen symbol lookup WBTC --output json

# Initialize WBTC exactly once with the operational token admin/deployer key.
# This creates wbtc_admin, wbtc_attester, wbtc_minter, wbtc_supply,
# wbtc_minted, wbtc_burned, wbtc_epoch_start, and wbtc_epoch_mint storage.
LICHEN_RPC_URL=https://testnet-api.lichen.network \
  ./target/release/lichen token initialize WBTC \
  --keypair /path/to/governed-deployer.json

# Confirm WBTC is initialized before wiring custody or public UI surfaces.
curl -fsS https://testnet-api.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getProgramStorage","params":["<live WBTC program>",{"limit":50}]}' \
  | jq -e '.result.entries | map(.key_decoded // .key) | any(. == "wbtc_admin")'

# Refresh operator manifests from the live symbol registry.
LICHEN_RPC_URL=https://testnet-api.lichen.network \
  python3 scripts/update-manifest.py

# On every custody host, fetch wrapped-token addresses and enforce the BTC route config.
sudo bash scripts/sync-custody-wrapped-contracts.sh \
  --rpc-url https://testnet-api.lichen.network \
  --env-file /etc/lichen/custody-env
sudo bash scripts/apply-custody-route-profile.sh \
  --profile /etc/lichen/custody-routes-testnet.env \
  --target /etc/lichen/custody-env \
  --routes solana,ethereum,bnb,neox,bitcoin
sudo bash scripts/verify-custody-routes.sh \
  --env-file /etc/lichen/custody-env \
  --routes solana,ethereum,bnb,neox,bitcoin \
  --require-wrapped
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

## Live v0.5.95 Testnet Execution Record

On 2026-06-05, the live testnet WBTC contract was deployed and registered without rewriting historical state:

- WBTC contract: `6zQChEy6XacfQR52892oAMpntavfpb6mBUvLRkyXxno1`;
- deployer/admin: `63XtXerx8x1w5HjQXPBPLE9Q6P3qyvWWuCEjdzMbAiC`;
- deploy transaction: `4a49145b6fe7e9e3cbc3fb5729a8f24eb8b9ec79e0042e6878319a68ccc81732`;
- registry symbol: `WBTC`, display symbol `wBTC`, template `wrapped`, decimals `9`;
- the then-live validator fleet ran `lichen-validator 0.5.95`;
- local BTC regtest custody smoke passed before VPS rollout;
- all public frontend portals were redeployed after the signed metadata refresh.

The live DEX additions are governed protocol contract-call proposals, not database edits, RPC shims, or frontend-only changes. They were proposed against the governed DEX admin authority and approved by four authorized genesis governance accounts. The proposals are timelocked until epoch 6, which starts at slot `2_592_000`.

| Proposal | Target | Action |
| --- | --- | --- |
| 2 | `dex_core` | create `wBTC/lUSD` CLOB pair with pair id 12 |
| 3 | `dex_core` | create `wBTC/LICN` CLOB pair with pair id 13 |
| 4 | `dex_amm` | create `wBTC/lUSD` AMM pool with pool id 12 |
| 5 | `dex_amm` | create `wBTC/LICN` AMM pool with pool id 13 |
| 6 | `dex_router` | register `wBTC/lUSD` CLOB route |
| 7 | `dex_router` | register `wBTC/lUSD` AMM route |
| 8 | `dex_router` | register `wBTC/LICN` CLOB route |
| 9 | `dex_router` | register `wBTC/LICN` AMM route |

The US seed runs a durable executor service:

```bash
systemctl status lichen-wbtc-dex-execute-testnet.service
tail -f /home/ubuntu/wbtc-dex-execute-epoch6.log
```

The service waits for epoch 6, then executes proposals `2..9` in order with the governed community treasury key. It is idempotent across restarts: if a proposal was already executed, it logs that state and continues. Governance contract-call executions must carry an explicit `--compute-budget 1400000`; for this rollout, the current `simulateTransaction` preflight path false-failed on the nested governed DEX admin calls, so final verification used finalized transaction status plus REST pair/pool/route state.

Epoch 6 execution finalized on 2026-06-06 with these transaction signatures:

| Proposal | Signature | Result |
| --- | --- | --- |
| 2 | `d0175a9043c66bf36516f78b0b26ffccbe572ad087884777b109c23ca26b5260` | `wBTC/lUSD` CLOB pair created |
| 3 | `5fa0d94669abfcccb368ba8238502fb1f42f664d6eca378281a7f5e196526ec8` | `wBTC/LICN` CLOB pair created |
| 4 | `7fd9c42887b84fe96b92a928c07f824656c12dadcaecfe9508f0e55ab3bdfe37` | `wBTC/lUSD` AMM pool created |
| 5 | `07c7007a25023ee314a9ba8f31f139a6a0720c17954ec1bd6899f87fee1356b6` | `wBTC/LICN` AMM pool created |
| 6 | `fdd64cff73c26b8e9f095fe33959fb204c07e4fdc8b3053cd90f519adb8feff6` | `wBTC/lUSD` CLOB route registered |
| 7 | `516d579eef02d8514a72ebe55e7a639ef7d3c70a0244e382dacd38f5352ee44e` | `wBTC/lUSD` AMM route registered |
| 8 | `2a49cde4a4bf8a28b1807f483a6beb92e41ded5897acc7f114a006ab7893f754` | `wBTC/LICN` CLOB route registered |
| 9 | `c5b5aeb5248f85e451a1415d3587d6de73f38d78f7d35fcd9843181369ec48a7` | `wBTC/LICN` AMM route registered |

Post-execution verification:

```bash
curl -s http://127.0.0.1:8899/api/v1/pairs | python3 -m json.tool
curl -s http://127.0.0.1:8899/api/v1/pools | python3 -m json.tool
curl -s http://127.0.0.1:8899/api/v1/routes | python3 -m json.tool
```

Expected visible state after execution:

- pair count increases from 11 to 13 and includes `wBTC/lUSD` and `wBTC/LICN`;
- pool count increases from 11 to 13 and includes the matching WBTC pools;
- route count increases from 22 to 26 and includes CLOB plus AMM routes for both WBTC pairs;
- `/api/v1/oracle/prices` continues to expose a live non-stale `wBTC` feed.

## Non-Goals

- No MossStake changes.
- No historical migration or root-changing replay behavior.
- No compatibility shim that pretends BTC custody works without a real Bitcoin route.
- No VPS mutation before local three-validator validation.
