# Custody Route Profile Runbook

This is the operator checklist for wiring source-chain custody routes after a
testnet fresh start, during live testnet repair, and before mainnet launch. It
covers external-chain route values only. Never commit private keys, auth tokens,
custody seeds, funded keypairs, or provider API secrets.

For the full 4-validator mainnet launch sequence, use
`deploy/mainnet-launch-runbook.md`. That runbook starts the Lichen
chain first, then starts custody only after post-genesis verification and
route-specific dust tests pass.

## What Genesis Creates

Genesis creates Lichen-side state: wrapped-token contracts, symbol registry
entries, custody treasury material, custody master seeds, the deposit derivation
seed, signer/password wiring, and mandatory CLOB pairs, AMM pools, and router
routes for all launch wrapped assets, including wNEO, wGAS, and wBTC.

Genesis does not create or choose Solana, Ethereum, BNB Chain, Neo X, or
Bitcoin RPC providers; source-chain token mints/contracts; source-chain gas or
fees; or the funded Solana fee payer. Those values live in
`/etc/lichen/custody-env`, normally merged from an operator-owned route profile.

If `CUSTODY_TREASURY_SOLANA`, `CUSTODY_TREASURY_ETH`,
`CUSTODY_TREASURY_BNB`, `CUSTODY_TREASURY_NEOX`, or `CUSTODY_TREASURY_BTC` are
unset, custody derives deterministic treasury addresses from the custody master
seed. Explicit treasury env vars are optional pins.

Neo X is an EVM-compatible sidechain, so Neo X treasury, deposit, multisig, and
token contract values use Ethereum-style `0x...` addresses. Do not use Neo N3
`N...` Base58 addresses for `CUSTODY_TREASURY_NEOX`, Neo X deposit
destinations, or Neo X token contracts. A Neo N3 bridge route would be a
separate integration with different address derivation and signing.

## Required Route Policy

Set required routes before a clean-slate deploy:

```bash
export CUSTODY_REQUIRED_ROUTES=solana,ethereum,bnb,neox,bitcoin
```

Use a smaller list only when a route is deliberately disabled. Use
`CUSTODY_REQUIRED_ROUTES=none` only for local development or a no-source-chain
custody drill. A fresh start should fail route verification before the wallet
can present a half-wired bridge.

## Live Testnet Profile

Current Lichen testnet uses Solana devnet plus controlled EVM source devchains.
This avoids public faucet/captcha gating while exercising the same custody
watch, sweep, and wrapped-credit code paths.

```bash
# Solana devnet.
CUSTODY_SOLANA_RPC_URL=https://api.devnet.solana.com
CUSTODY_SOLANA_CONFIRMATIONS=32
CUSTODY_SOLANA_FEE_PAYER=/etc/lichen/secrets/solana-fee-payer-testnet.json
CUSTODY_SOLANA_USDC_MINT=HkQm3La88aPgs9xSrSGW6KowTxAwN5vQm18Rwgr17vmL
CUSTODY_SOLANA_USDT_MINT=GeY6rgTWxxqnJSsWh79Gg58aShfUDHUrMrLCmkEjxZEe

# Controlled Ethereum source devchain, Sepolia chain-id profile.
CUSTODY_ETH_RPC_URL=http://15.204.229.189:18545
CUSTODY_ETH_CHAIN_ID=11155111
CUSTODY_EVM_CONFIRMATIONS=12
CUSTODY_ETH_USDC_TOKEN_ADDR=0xe78A0F7E598Cc8b0Bb87894B0F60dD2a88d6a8Ab
CUSTODY_ETH_USDT_TOKEN_ADDR=0x5b1869D9A4C187F2EAa108f3062412ecf0526b24

# Controlled BSC source devchain, BSC-testnet chain-id profile.
CUSTODY_BNB_RPC_URL=http://15.204.229.189:18546
CUSTODY_BNB_CHAIN_ID=97
CUSTODY_BSC_USDC_TOKEN_ADDR=0xe78A0F7E598Cc8b0Bb87894B0F60dD2a88d6a8Ab
CUSTODY_BSC_USDT_TOKEN_ADDR=0x5b1869D9A4C187F2EAa108f3062412ecf0526b24

# Existing Neo X route used by the live testnet.
CUSTODY_NEOX_RPC_URL=https://mainnet-1.rpc.banelabs.org
CUSTODY_NEOX_CHAIN_ID=47763
CUSTODY_NEOX_CONFIRMATIONS=12
CUSTODY_NEOX_NEO_TOKEN_ADDR=0xc28736dc83f4fd43d6fb832Fd93c3eE7bB26828f

# Bitcoin test route. Use testnet or regtest for non-mainnet drills.
CUSTODY_BTC_RPC_URL=REPLACE_WITH_BITCOIN_TEST_RPC_URL
CUSTODY_BTC_RPC_USER=REPLACE_WITH_BITCOIN_RPC_USER
CUSTODY_BTC_RPC_PASSWORD=REPLACE_WITH_BITCOIN_RPC_PASSWORD
CUSTODY_BTC_NETWORK=testnet
CUSTODY_BTC_CONFIRMATIONS=6
CUSTODY_BTC_FEE_RATE_SATS_VB=5
CUSTODY_TREASURY_BTC=REPLACE_WITH_TESTNET_BTC_TREASURY_ADDRESS
```

Controlled source devchains run on the genesis/US host:

```text
lichen-eth-source-devchain.service      # port 18545, chain id 11155111
lichen-bnb-source-devchain.service      # port 18546, chain id 97
lichen-source-devchain-miner.service    # evm_mine every 2 seconds
/opt/lichen-source-devchains/deployed-routes.json
```

The miner must mine both EVM devchain ports. In systemd, quote the multi-value
environment entry; otherwise only the first port may be mined and BSC
withdrawals will sit below the required confirmation depth:

```ini
[Service]
Environment=LICHEN_SOURCE_DEVCHAIN_MINE_INTERVAL=2
Environment="LICHEN_SOURCE_DEVCHAIN_PORTS=18545 18546"
```

Allow the joining custody hosts to reach ports `18545` and `18546`. Do not
point public users at these devchains; they are operator test infrastructure.

## Mainnet Profile

For mainnet, replace the controlled/test endpoints with production RPC
providers and real token contracts. Do not use unauthenticated public RPC
endpoints for production custody.

```bash
# Solana mainnet.
CUSTODY_SOLANA_RPC_URL=REPLACE_WITH_SOLANA_MAINNET_RPC_URL
CUSTODY_SOLANA_CONFIRMATIONS=32
CUSTODY_SOLANA_FEE_PAYER=/etc/lichen/secrets/solana-fee-payer-mainnet.json
CUSTODY_SOLANA_USDC_MINT=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
CUSTODY_SOLANA_USDT_MINT=Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB

# Ethereum mainnet.
CUSTODY_ETH_RPC_URL=REPLACE_WITH_ETHEREUM_MAINNET_RPC_URL
CUSTODY_ETH_CHAIN_ID=1
CUSTODY_EVM_CONFIRMATIONS=12
CUSTODY_ETH_USDC_TOKEN_ADDR=0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48
CUSTODY_ETH_USDT_TOKEN_ADDR=0xdAC17F958D2ee523a2206206994597C13D831ec7

# BNB Chain mainnet. The USDC/USDT values below are Binance-Peg BEP-20
# examples, not native Circle/Tether redemption routes. Treat them as an
# explicit asset-policy choice and verify issuer/explorer references before
# launch. These BEP-20 assets use 18 decimals.
CUSTODY_BNB_RPC_URL=REPLACE_WITH_BSC_MAINNET_RPC_URL
CUSTODY_BNB_CHAIN_ID=56
CUSTODY_BSC_USDC_TOKEN_ADDR=0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d
CUSTODY_BSC_USDT_TOKEN_ADDR=0x55d398326f99059fF775485246999027B3197955

# Neo X mainnet. Values in this section are EVM/0x-format values, not Neo N3
# N-addresses. Neo X mainnet chain id is 47763; Neo X Testnet T4 is 12227332.
CUSTODY_NEOX_RPC_URL=REPLACE_WITH_NEO_X_MAINNET_RPC_URL
CUSTODY_NEOX_CHAIN_ID=47763
CUSTODY_NEOX_CONFIRMATIONS=12
CUSTODY_NEOX_NEO_TOKEN_ADDR=REPLACE_WITH_NEO_X_NEO_CONTRACT

# Bitcoin mainnet.
CUSTODY_BTC_RPC_URL=REPLACE_WITH_BITCOIN_MAINNET_RPC_URL
CUSTODY_BTC_RPC_USER=REPLACE_WITH_BITCOIN_RPC_USER
CUSTODY_BTC_RPC_PASSWORD=REPLACE_WITH_BITCOIN_RPC_PASSWORD
CUSTODY_BTC_NETWORK=mainnet
CUSTODY_BTC_CONFIRMATIONS=6
CUSTODY_BTC_FEE_RATE_SATS_VB=5
CUSTODY_TREASURY_BTC=REPLACE_WITH_BTC_TREASURY_ADDRESS
```

Before mainnet launch, verify token decimals and issuer policy against primary
sources, then run the same deposit/sweep/credit smoke tests with dust-sized
amounts and a production release binary.

Neo X route caveat: the current verifier treats `neox` as requiring both the
GAS route and `CUSTODY_NEOX_NEO_TOKEN_ADDR`. Neo X is EVM-compatible, so the
address format is correctly `0x...`, but do not use an unverified NEO token
contract only to satisfy the verifier. If the public launch scope is Neo X GAS
only, either keep Neo X custody non-public until the NEO contract is approved or
ship a route verifier update that can represent a GAS-only Neo X route.

## Apply And Verify

Keep filled profiles outside the repo:

```text
/etc/lichen/custody-routes-testnet.env
/etc/lichen/custody-routes-mainnet.env
```

Recommended permissions:

```bash
sudo chown root:root /etc/lichen/custody-routes-testnet.env
sudo chmod 600 /etc/lichen/custody-routes-testnet.env
```

Apply a profile:

```bash
sudo bash scripts/apply-custody-route-profile.sh \
  --profile /etc/lichen/custody-routes-testnet.env \
  --target /etc/lichen/custody-env \
  --routes "$CUSTODY_REQUIRED_ROUTES"
```

On runtime-only VPSes without a repo checkout, use the installed copies under
`/opt/lichen/ops-scripts`.

After genesis, refresh the Lichen-side wrapped-token pins from the live symbol
registry. These pins change after every fresh chain reset.

```bash
sudo bash scripts/sync-custody-wrapped-contracts.sh \
  --env-file /etc/lichen/custody-env \
  --rpc-url http://127.0.0.1:8899
```

Verify without printing configured secrets:

```bash
sudo bash scripts/verify-custody-routes.sh \
  --env-file /etc/lichen/custody-env \
  --routes "$CUSTODY_REQUIRED_ROUTES" \
  --require-wrapped
```

Restart only after verification succeeds:

```bash
sudo systemctl restart lichen-custody
sudo systemctl status lichen-custody --no-pager
```

VPS custody hosts are runtime-only. If source-chain code changes are required,
build and release the custody binary elsewhere, copy it to
`/usr/local/bin/lichen-custody`, and restart the service. Do not plan on
rebuilding Rust on the hosts. For withdrawal smoke tests that use normal CLI
JSON contract calls, the validator/RPC release must include the wrapped-token
burn metadata parser that recognizes JSON, layout-descriptor, and raw-binary
call arguments. Until that validator/RPC release is rolled everywhere, use the
`wrapped_burn` helper described below because it submits the raw call format the
current live RPC metadata parser already reports correctly.

## Fresh-Start Sequence

1. Run genesis and post-genesis deployment.
2. Create `/etc/lichen/custody-routes-testnet.env` or the mainnet equivalent.
3. Install the Solana fee-payer keypair as root-readable and `lichen`
   group-readable, for example `0640 root:lichen`.
4. Fund the Solana fee payer with enough SOL for ATA creation and SPL sweeps.
5. Create or confirm Solana SPL test mints and fund the operator source wallet.
6. Start controlled source devchains and the miner service for testnet, or
   configure production RPCs for mainnet.
7. Apply the route profile.
8. Run `scripts/sync-custody-wrapped-contracts.sh`.
9. Run `scripts/verify-custody-routes.sh --require-wrapped`.
10. Deploy a custody binary containing the current route fixes.
11. Restart custody and check `/health`.
12. Run functional deposit, sweep, and credit smoke tests for every live route.

For a three-host testnet, repeat profile application, wrapped-pin sync, route
verification, binary rollout, and service restart on all custody VPSes.
`scripts/clean-slate-redeploy.sh` performs the apply/sync/verify steps on the
genesis host when `/etc/lichen/custody-routes-<network>.env` exists, bundles the
resulting runtime env to the joiners, and verifies joiners with
`--require-wrapped`.

## Funding Requirements

Solana:

- `CUSTODY_SOLANA_FEE_PAYER` must hold SOL for ATA creation and token sweeps.
- The derived or explicit Solana treasury must be the owner used for treasury
  token accounts.
- SPL test mints must match `CUSTODY_SOLANA_USDC_MINT` and
  `CUSTODY_SOLANA_USDT_MINT`.

Ethereum and BNB Chain:

- Source treasury/deposit sweep execution needs native ETH or BNB for gas.
- `CUSTODY_ETH_CHAIN_ID` and `CUSTODY_BNB_CHAIN_ID` must match the RPC endpoint.
- Ethereum stablecoin routes are treated as 6-decimal assets; BSC stablecoin
  routes are treated as 18-decimal BEP-20 assets.
- The devchain miner must be active or EVM confirmation polling will stall.

Neo X:

- `CUSTODY_NEOX_RPC_URL`, `CUSTODY_NEOX_CHAIN_ID`,
  `CUSTODY_NEOX_CONFIRMATIONS`, and `CUSTODY_NEOX_NEO_TOKEN_ADDR` must match
  the selected Neo X network.
- Neo X GAS is the native EVM gas asset and uses the derived or explicit
  `0x...` Neo X treasury for sweep and withdrawal gas. Neo X NEO is an EVM token
  route configured through `CUSTODY_NEOX_NEO_TOKEN_ADDR`; it is not a Neo N3
  `N...` address route.

Bitcoin:

- `CUSTODY_BTC_RPC_URL`, `CUSTODY_BTC_NETWORK`,
  `CUSTODY_BTC_CONFIRMATIONS`, and `CUSTODY_BTC_FEE_RATE_SATS_VB` must match
  the selected Bitcoin network.
- Mainnet BTC deposits and withdrawals use native SegWit `bc1...` addresses.
  Testnet and regtest use the matching `tb1...` or `bcrt1...` HRP.
- The BTC treasury needs spendable UTXOs for withdrawals; sweep fees are paid
  from the swept deposit amount.

## Treasury Movement Policy

Do not treat a custody treasury as an operator hot wallet. The custody service
has no generic "send treasury funds to admin/genesis" API. Supported automated
egress is limited to:

1. user redemptions after a matching Lichen wrapped-token burn is verified,
2. configured stablecoin reserve rebalances, and
3. documented incident or recovery actions performed outside the public custody
   API.

Standard user redemptions are not epoch-timelocked. They are protected by the
signed withdrawal request, replay guard, wrapped-token burn verification,
route/restriction checks, velocity caps, optional signer quorum, optional
operator confirmation for extraordinary withdrawals, and the bridge incident
pause. Elevated and extraordinary redemptions can add delay and confirmation
requirements, but this is custody velocity policy, not Lichen governance.

Protocol treasury movements on Lichen, such as moving protocol funds to a
genesis/admin wallet, must use the governed `TreasuryTransfer` path with the
`treasury_executor` approval authority, its threshold, its timelock, and its
daily-cap policy. Operator admin tokens and service-control credentials are not
governance keys.

External-chain custody treasuries are different from the Lichen protocol
treasury: Lichen governance can authorize the policy decision, but Solana/EVM
execution is enforced by the source chain's signer architecture. For mainnet,
manual custody treasury movements must be backed by the configured threshold,
Safe, HSM, MPC, or equivalent operator-controlled procedure and recorded with
proposal/signoff evidence. Do not move external custody treasury funds to an
admin/genesis wallet by ad hoc use of the raw custody seed.

## Functional Smoke Test

Use the RPC auth payload helper to create route deposits without exposing
custody service tokens:

```bash
cargo run -p lichen-rpc --bin bridge_auth_payload -- \
  --chain solana --asset usdc --seed-byte 42 --ttl-secs 3600 \
  > /tmp/bridge-auth-solana-usdc.json

curl -fsS https://testnet-rpc.lichen.network \
  -H 'Content-Type: application/json' \
  --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"createBridgeDeposit\",\"params\":[$(cat /tmp/bridge-auth-solana-usdc.json)]}"
```

For each live source route:

1. Create a deposit through `createBridgeDeposit`.
2. Send a small source-chain amount to the returned address.
3. Poll `getBridgeDeposit` with the same auth payload.
4. Require status progression to `credited`.
5. Confirm the source deposit address no longer holds the swept asset.
6. Confirm custody `/status` and logs have no new permanent failures.

On 2026-05-30, the live testnet smoke test reached `credited` for:

```text
solana:sol, solana:usdc, solana:usdt
ethereum:eth, ethereum:usdc, ethereum:usdt
bsc:bnb, bsc:usdc, bsc:usdt
```

## Withdrawal Smoke Test

Use small wrapped-token burns to prove the reverse path after deposit credits.
The helper binaries keep the authorization key, destination, burn call, and
amount format repeatable:

```bash
# Optional deterministic test identity.
cargo run -p lichen-rpc --bin keypair_from_seed_byte -- \
  --seed-byte 81 --output /tmp/lichen-withdraw-user.json

# Signed custody withdrawal authorization payload.
cargo run -p lichen-rpc --bin withdrawal_auth_payload -- \
  --asset wsol \
  --amount 1000000 \
  --dest-chain solana \
  --dest-address REPLACE_WITH_SOLANA_DESTINATION \
  --seed-byte 81 \
  --ttl-secs 3600 \
  > /tmp/withdrawal-auth-wsol.json

# Burn the wrapped asset on Lichen using the raw call format.
cargo run -p lichen-rpc --bin wrapped_burn -- \
  --rpc-url https://testnet-rpc.lichen.network \
  --contract "$CUSTODY_WSOL_TOKEN_ADDR" \
  --amount 1000000 \
  --seed-byte 81
```

Operational sequence:

1. Ensure the source-chain treasury has enough native gas and the Solana fee
   payer has SOL.
2. Create the withdrawal with `POST /withdrawals` using the JSON emitted by
   `withdrawal_auth_payload`.
3. Burn the exact wrapped amount with `wrapped_burn`.
4. Submit the burn signature with `PUT /withdrawals/:job_id/burn`.
5. Poll custody events for `withdrawal.burn_confirmed`,
   `withdrawal.broadcast`, and `withdrawal.confirmed`.
6. Query `GET /withdrawals/:job_id` and require `"status":"confirmed"`.
7. Confirm the source-chain destination transaction is successful/finalized.

On 2026-05-30, the live testnet withdrawal drill completed for:

```text
weth -> ethereum: job da8d5304-774f-47e0-be6a-c68d0341606d
  burn bd0baf89cbaeea7a896990ba81c2520c6c34ab3bce46c078f858d73c5dd5f16b
  outbound 0xf934cda5108edaf91175ee5b2212218909fc6c2ee28de173ac27782706a8c86a

wbnb -> bsc: job e4e01e47-ab03-4730-8acd-219f8eb88d73
  burn bce94bbd4a2327a00349096273592494a50ce2fb4fdbafe3bc5ee552c17a4a0a
  outbound 0xcd3d132d85cd3b95347955b08d6c0046d1504b46ad9ea2ea9ac9405f360ecd36

wsol -> solana: job 93bf72ff-97da-4f99-b0b3-362b67e17379
  burn 44cbe8f85bbac4deaf5a65d873e1f096af57acf7475b471f0cb1c0253cec6244
  outbound 4FBJE741zXEdiKTZaHPDNph6cDKxrvFCLBiEkAboo93dyFy4EdMZRWcJvkM7mjJ1cQoeGZCUhucKHMuHQFoadpP2
```

Keep a source-chain custody route out of `CUSTODY_REQUIRED_ROUTES` and hide it
from wallet surfaces until this smoke test passes. This does not remove or make
optional the genesis-native wrapped token contract, DEX pair, AMM pool, or
router route.
