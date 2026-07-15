# Lichen Mainnet Launch Runbook

This is the operator runbook for launching Lichen mainnet and then enabling
mainnet custody. It is intentionally step-by-step and gate-based. Do not skip a
gate because mainnet genesis and custody routes handle real value.

Written for the current mainnet package. Candidate release target for this
runbook is `v0.5.224`; keep `v0.5.223` as the signed rollback point. The
candidate is not deployable until it passes CI, archive parity, signature
verification, and deployment approval.

## Operating Rules

- Planning and pre-staging are reversible. Genesis is not casual.
- Mainnet validator services may be installed while disabled/inactive before
  genesis. That is expected.
- Start the Lichen chain first. Enable custody only after all four validators
  are healthy, synced, and serving the same genesis.
- Public mainnet validators must be archive-backed from first boot. `v0.5.190`
  and later refuse non-dev `mainnet` startup unless `--archive-mode` and
  `--cold-store /var/lib/lichen/archive-mainnet` are both present.
- Every mainnet block after height 1 must contain exactly one version-2
  canonical parent commit transaction at index 0. Sync and startup must verify
  its signatures against the complete parent-height powers authenticated by the
  parent `validators_hash`; a missing power snapshot, envelope, parent body, or
  two-thirds threshold is fatal and must not be bypassed.
- `getBlockCommit` on every validator must return the same `canonical_child`
  certificate version, `validators_hash`, validator powers, round, and
  signatures at a fixed non-tip slot. Local pending-tip evidence is not archive
  parity or a mainnet launch proof.
- Do not expose public custody or wallet routes until that exact route passes a
  dust deposit and dust withdrawal on mainnet.
- Do not use the mainnet faucet pattern. There is no mainnet faucet.
- Do not copy validator RocksDB state, `genesis-wallet.json`, `genesis-keys/`,
  `known-peers.json`, or consensus WAL to joiners.
- Do not delete or unmount the original `genesis-keys/` governed signer bundle
  until `scripts/verify-governed-key-custody.sh` has passed against the live
  chain and at least two private/offline backup copies have been verified.
  The genesis primary key is one governed signer; it is not a unilateral bypass
  for distribution wallets, signer rotation, treasury execution, or timelocks.
- Do not carry testnet lineage recovery hooks into a mainnet launch unless they
  are provably unreachable for mainnet by chain ID, genesis wallet identity, and
  activation slot. The `testnet_governed_signer_recovery_v1` hook exists only to
  preserve replay compatibility for the June 2026 testnet after governed signer
  custody was lost; mainnet must launch from verified custody instead.
- Do not deploy a release that changes consensus rules with a mixed-version
  rolling restart. The current rollback point `v0.5.223` must remain available
  until a newer signed rollback point is explicitly recorded.
- Do not commit provider URLs, auth tokens, keypair passwords, custody seeds,
  funded keypairs, signing keys, or filled production env files.
- Do not print secrets in shell logs, tickets, chat, or launch notes. Print key
  names, file paths, hashes, public addresses, and transaction hashes only.
- A custody treasury is not an admin hot wallet. User redemptions use the burn
  and custody release flow. Protocol treasury movements use governance,
  timelock, threshold, and audit evidence.

## Current Host Shape

Current four-validator VPS set:

| Role | IP | Expected mainnet services |
| --- | --- | --- |
| US seed / primary | `15.204.229.189` | `lichen-validator-mainnet`, `lichen-custody-mainnet` |
| EU joiner | `37.59.97.61` | `lichen-validator-mainnet`, `lichen-custody-mainnet` |
| SEA joiner | `15.235.142.253` | `lichen-validator-mainnet`, `lichen-custody-mainnet` |
| IN joiner / seed-04 | `148.113.43.247` | `lichen-validator-mainnet`, `lichen-custody-mainnet` |

Mainnet ports and public endpoints:

| Surface | Value |
| --- | --- |
| P2P | `8001/tcp`, `8001/udp` |
| Local RPC | `127.0.0.1:9899` |
| Local WebSocket | `127.0.0.1:9900` |
| Local custody | `127.0.0.1:9106` |
| Public RPC | `https://rpc.lichen.network` |
| Public WebSocket | `wss://rpc.lichen.network/ws` |
| Public custody | `https://custody.lichen.network` only after custody gates pass |

Expected mainnet filesystem paths:

| Path | Purpose |
| --- | --- |
| `/etc/lichen/env-mainnet` | validator and public RPC env |
| `/etc/lichen/custody-env-mainnet` | custody runtime env |
| `/etc/lichen/custody-routes-mainnet.env` | operator-owned source route profile |
| `/etc/lichen/secrets/custody-master-seed-mainnet.txt` | custody treasury derivation seed |
| `/etc/lichen/secrets/custody-deposit-seed-mainnet.txt` | custody deposit derivation seed |
| `/etc/lichen/secrets/solana-fee-payer-mainnet.json` | funded Solana fee payer |
| `/etc/lichen/custody-treasury-mainnet.json` | Lichen-side custody treasury keypair |
| `/etc/lichen/signed-metadata-manifest-mainnet.json` | signed manifest |
| `/var/lib/lichen/state-mainnet` | validator state |
| `/var/lib/lichen/custody-db-mainnet` | custody RocksDB |

## Source Facts To Reconfirm

Reconfirm these from primary sources before launch day execution:

- Circle USDC contract list:
  `https://developers.circle.com/stablecoins/usdc-contract-addresses`
- Tether supported protocols:
  `https://tether.to/en/supported-protocols/`
- BNB Chain RPC and chain ID:
  `https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/`
- Neo X network and bridge docs:
  `https://xdocs.ngd.network/bridge/quick-start-bridging-assets`

Known public constants as of 2026-05-31:

| Route | Value |
| --- | --- |
| Solana USDC mint | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| Solana USDT mint | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` |
| Ethereum chain ID | `1` |
| Ethereum USDC | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` |
| Ethereum USDT | `0xdAC17F958D2ee523a2206206994597C13D831ec7` |
| BNB Smart Chain ID | `56` |
| BSC Binance-Peg USDC candidate | `0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d` |
| BSC Binance-Peg BSC-USD/USDT candidate | `0x55d398326f99059fF775485246999027B3197955` |
| Neo X mainnet chain ID | `47763` |
| Neo X mainnet currency symbol | `GAS` |

BNB route policy gate:

- Circle's official USDC page does not list BNB Smart Chain as native USDC in
  the same way it lists Ethereum and Solana.
- Tether's page lists BNB Smart Chain as an ERC20-compatible protocol, but the
  commonly used BSC `0x55d398...` route is exposed operationally as
  Binance-Peg BSC-USD on BscScan.
- Therefore BNB stablecoin routes require an explicit product and risk approval
  that they are Binance-Peg BEP-20 assets, not native issuer-redemption routes.

Neo X route policy gate:

- Neo X is EVM-compatible, so Neo X addresses are `0x...` EVM addresses. Do not
  use Neo N3 `N...` addresses for `CUSTODY_TREASURY_NEOX`, Neo X deposits,
  Neo X destinations, or Neo X token contracts.
- The current Neo X public bridge docs describe GAS bridging and say the bridge
  currently supports GAS. They do not establish a day-one NEO token route by
  themselves.
- Fresh genesis still includes wNEO, wGAS, wBTC, and all 13 launch DEX
  pairs/pools/routes. The policy gate below controls public source-chain
  custody activation only; it is not a DEX market optionality switch.
- The custody verifier currently treats `CUSTODY_REQUIRED_ROUTES=neox` as a
  requirement for both `wGAS` and `wNEO`, because it requires
  `CUSTODY_NEOX_NEO_TOKEN_ADDR`.
- Do not fill `CUSTODY_NEOX_NEO_TOKEN_ADDR` with an unverified address just to
  make the verifier pass. Either launch Neo X custody with an approved official
  NEO token contract and dust test both `wGAS` and `wNEO`, or keep Neo X custody
  non-public until that route policy is resolved.

## Launch Gates

Every gate must be recorded in the launch log with timestamp, operator, host,
command summary, and result.

| Gate | Result required |
| --- | --- |
| Release gate | CI green, signed GitHub Release exists, `SHA256SUMS` and signature verified |
| Host gate | all four VPSes have expected binaries, service files, env files, Caddy config, firewall |
| State gate | no stale mainnet chain state or custody DB unless explicitly approved |
| External route gate | production RPCs, token/mint addresses, chain IDs, and asset policy approved |
| Key gate | validator keys, custody seeds, signer tokens, signing keys, Solana fee payer, treasury keys installed with correct ownership |
| Funding gate | Solana fee payer has SOL; source-chain treasuries have enough native gas for dust tests |
| Genesis gate | genesis hash recorded; seed produces blocks; joiners sync from peers |
| Post-genesis gate | wrapped contract pins synced; signed metadata installed; route verifier passes |
| Custody health gate | custody `/health`, `/status`, logs, and route readiness pass |
| Route smoke gate | every public route passes dust deposit and dust withdrawal |
| Public gate | only passed routes are enabled in wallet/frontend/public docs |

Clean-slate invariants for the current package:

- Metadata must expose 32 manifest symbols before any public frontend deploy.
- Genesis must include the mandatory 13 DEX CLOB pairs, AMM pools, and router routes, including `wBTC/lUSD` and `wBTC/LICN`.
- Checkpoint serving uses RocksDB read-only descriptors and cannot cold-rebuild or compact checkpoint Merkle state from the serving path.
- Keep checkpoint disk retention bounded with `LICHEN_CHECKPOINT_MAX_BYTES`; use the release default unless an operator deliberately documents a larger cap.
- A resuming validator requests catch-up block ranges from one primary peer per chunk with fallback, avoiding duplicate range floods while preserving replay from peers.
- Warp and repair snapshots require authenticated PQ node sources plus a
  self-contained canonical proof: parent certificate, transaction-0 Merkle
  inclusion in the child, signed/finalized child header, historical child powers,
  and a certificate-bound parent post-state root. No reserved-seed trust bypass is allowed.
- Abort a snapshot source when the deterministic archive manifest differs from the verified checkpoint metadata.
- Every validator env must pin the other validator P2P endpoints in
  `LICHEN_P2P_RESERVED_PEERS`, excluding its own endpoint, so reconnect pressure
  preserves a full validator mesh after restarts and clean rejoins.

Hard stop conditions:

- Any host runs a binary hash that does not match the signed release package.
- Any host has an unexpected non-empty mainnet state directory before genesis.
- `rg "TESTNET_GOVERNED_SIGNER_RECOVERY|testnet_governed_signer_recovery" validator/src core/src`
  finds a mainnet-reachable recovery path or any recovery hook that is not
  chain-id and wallet guarded.
- Mainnet genesis prices cannot be sourced from an audited file or live provider.
- Fewer than four unique validator pubkeys are embedded in bridge/oracle
  genesis committees.
- A joiner requires copied chain state to sync.
- Any source RPC returns the wrong chain ID.
- `verify-custody-routes.sh` fails for a route intended to go public.
- Public RPC reports stale or divergent health.
- A dust deposit or withdrawal does not settle cleanly.
- Operators cannot explain where source-chain treasury funds are held and how
  threshold approval works.

## Phase 0: Local Launch Packet

Create a local launch packet directory outside the repo:

```bash
mkdir -p ~/lichen-mainnet-launch-$(date +%Y%m%d)
cd ~/lichen-mainnet-launch-$(date +%Y%m%d)
touch launch-log.md
```

Record:

- release tag and commit SHA
- GitHub Actions CI URL
- release URL
- `SHA256SUMS` hash
- release signer address
- four VPS IPs and hostnames
- planned genesis time
- planned validator public keys after generated
- planned source custody routes
- all final go/no-go decisions

The launch log may contain public addresses and transaction hashes. It must not
contain private keys, auth tokens, seed material, provider URLs with embedded
credentials, or keypair passwords.

## Phase 1: Release Verification

Use the signed release that passed CI. For the current package:

```bash
export LICHEN_RELEASE_TAG=v0.5.224
export LICHEN_MAINNET_VPS_HOSTS="15.204.229.189 37.59.97.61 15.235.142.253 148.113.43.247"
```

Required release checks:

```bash
git fetch origin --tags
git rev-parse "$LICHEN_RELEASE_TAG"
git tag -v "$LICHEN_RELEASE_TAG" || true
gh run list --branch main --limit 10
gh release view "$LICHEN_RELEASE_TAG" --json url,isDraft,isPrerelease,assets
```

Expected release assets:

- `lichen-validator-linux-x86_64.tar.gz`
- `lichen-validator-linux-aarch64.tar.gz`
- `lichen-validator-darwin-x86_64.tar.gz`
- `lichen-validator-darwin-aarch64.tar.gz`
- `lichen-validator-windows-x86_64.tar.gz`
- `SHA256SUMS`
- `SHA256SUMS.sig`

Verify release archives locally before copying or deploying:

```bash
REPO_ROOT="$(git rev-parse --show-toplevel)"
mkdir -p /tmp/lichen-release-mainnet
gh release download "$LICHEN_RELEASE_TAG" \
  --dir /tmp/lichen-release-mainnet \
  --clobber
cd /tmp/lichen-release-mainnet
node "$REPO_ROOT/scripts/verify-release-checksums.mjs" .
sha256sum -c SHA256SUMS
```

The script requires `LICHEN_RELEASE_TAG` and must install the signed release archive
that passed CI; do not install unsigned or locally modified binaries.
The rolling deploy script verifies the detached PQ
signature over `SHA256SUMS` before it installs anything on a VPS. Do not bypass
that gate during rollout.

For an emergency rollback to the current signed rollback point, set the tag
explicitly and run the same signed-release path:

```bash
export LICHEN_RELEASE_TAG=v0.5.223
LICHEN_VERIFY_RELEASE_ONLY=1 bash scripts/rolling-release-deploy.sh mainnet
bash scripts/rolling-release-deploy.sh mainnet
```

Do not reset RocksDB state for a code rollback unless a separate incident
decision explicitly approves a destructive recovery.

The release must be signed by the configured release-signing key. For the
current trust anchor, the signer address is:

```text
8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk
```

This signer must match the key used to create `SHA256SUMS.sig` and the signed
metadata manifest. Do not rotate the public trust anchor unless the matching
private signing key is staged offline and every client trust table is updated
in the same release.

Run the trust-anchor drift gate before every release and before every frontend
publish:

```bash
node scripts/qa/test_release_signer_trust_anchor.js
```

Do not proceed if release verification fails.

### Signed Metadata And Frontend Gate

The DEX, wallet, programs, marketplace, explorer, monitoring, faucet, and
developers frontends resolve contract symbols through the signed metadata
manifest. They fail closed if the manifest signer is not the configured release
trust anchor. Before deploying Cloudflare Pages or rolling validators, verify
the public RPC serves a manifest signed by the current trust anchor and includes
the DEX-critical symbols:

```bash
EXPECTED_RELEASE_SIGNER="8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk"
RPC_URL="https://rpc.lichen.network"

curl -fsS "$RPC_URL" \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' \
  | EXPECTED_RELEASE_SIGNER="$EXPECTED_RELEASE_SIGNER" python3 -c '
import json, os, sys
d=json.load(sys.stdin)["result"]
p=d.get("payload") or {}
symbols={e.get("symbol") for e in p.get("symbol_registry", [])}
required={"DEX","DEXAMM","DEXROUTER","DEXMARGIN","DEXREWARDS","DEXGOV","ANALYTICS","PREDICT","LUSD"}
missing=sorted(required-symbols)
print({"signer": d.get("signer"), "symbols": len(symbols), "missing": missing})
assert d.get("signer") == os.environ["EXPECTED_RELEASE_SIGNER"]
assert not missing
'
```

For testnet, use `RPC_URL="https://testnet-api.lichen.network"` and keep the
same signer unless the release signer has been intentionally rotated in code,
runbooks, signed metadata, and release assets together.

If this gate fails, do not deploy the frontend and do not roll validators. Fix
the mismatch by regenerating signed metadata with the current release key, or
by performing a deliberate signer rotation across the release key, all client
trust tables, the validator updater, runbooks, and release signatures in one
validated release.

After the metadata gate passes, run the frontend asset gate before publishing
any portal:

```bash
node scripts/qa/test_frontend_asset_integrity.js
```

For DEX publishes, also verify the custom domain sees the newly versioned
metadata-critical assets. Do not rely on the `.pages.dev` preview alone:

```bash
DEX_URL="https://dex.lichen.network"
DEX_ASSET_VERSION="$(grep -o 'dex.js?v=[0-9]*' dex/index.html | head -1 | cut -d= -f2)"

curl -fsSL "$DEX_URL/index.html" | grep "shared/utils.js?v=$DEX_ASSET_VERSION"
curl -fsSL "$DEX_URL/index.html" | grep "shared-config.js?v=$DEX_ASSET_VERSION"
curl -fsSL "$DEX_URL/index.html" | grep "dex.js?v=$DEX_ASSET_VERSION"
curl -fsSL "$DEX_URL/shared/utils.js?v=$DEX_ASSET_VERSION" | grep "$EXPECTED_RELEASE_SIGNER"
curl -fsSI "$DEX_URL/shared/utils.js?v=$DEX_ASSET_VERSION" | grep -i '^cache-control:'
```

Hard stop if the custom domain still references an old `v=` token, the live
`shared/utils.js` does not contain the current signer, or the DEX signed
metadata smoke cannot resolve `DEX`, `DEXAMM`, `DEXROUTER`, `DEXMARGIN`,
`DEXREWARDS`, `DEXGOV`, `ANALYTICS`, and `PREDICT`. If Cloudflare applies a
zone-level cache rule with a positive JavaScript TTL, either purge/disable that
rule before public release or bump every metadata-critical asset token and
record the cache-control evidence in the launch log. A stale cached frontend
can hide a correct live symbol registry and fail closed with missing DEX
contracts; that is a frontend deployment failure, not a reason to reset chain
state.

## Phase 2: Host Preflight

Run this from the local operator machine. It prints versions and service state,
not secrets.

```bash
for host in $LICHEN_MAINNET_VPS_HOSTS; do
  echo "== $host =="
  ssh -p 2222 "ubuntu@$host" '
    set -e
    hostname
    uname -m
    command -v lichen-validator || true
    /usr/local/bin/lichen-validator --version || true
    sha256sum /usr/local/bin/lichen-validator /usr/local/bin/lichen-custody 2>/dev/null || true
    systemctl is-enabled lichen-validator-mainnet 2>/dev/null || true
    systemctl is-active lichen-validator-mainnet 2>/dev/null || true
    systemctl is-enabled lichen-custody-mainnet 2>/dev/null || true
    systemctl is-active lichen-custody-mainnet 2>/dev/null || true
    ls -ld /etc/lichen /etc/lichen/secrets /var/lib/lichen 2>/dev/null || true
    ls -l /etc/lichen/env-mainnet /etc/lichen/custody-env-mainnet /etc/lichen/seeds.json 2>/dev/null || true
    sudo test ! -e /var/lib/lichen/state-mainnet || sudo find /var/lib/lichen/state-mainnet -maxdepth 1 -mindepth 1 -print
    sudo test ! -e /var/lib/lichen/custody-db-mainnet || sudo find /var/lib/lichen/custody-db-mainnet -maxdepth 1 -mindepth 1 -print
  '
done
```

Expected before genesis:

- `lichen-validator-mainnet` may be disabled or inactive.
- `lichen-custody-mainnet` may be disabled or inactive.
- `/etc/lichen/env-mainnet` exists and is root-owned.
- `/etc/lichen/custody-env-mainnet` exists and is root-owned.
- `/etc/lichen/seeds.json` exists.
- `/var/lib/lichen/state-mainnet` is missing or empty.
- `/var/lib/lichen/custody-db-mainnet` is missing or empty.

If setup is missing, run this on each VPS from the checked-out release tree:

```bash
sudo bash deploy/setup.sh mainnet
```

Then repeat the host preflight. Do not continue until every host has the same
runtime shape.

## Phase 3: Mainnet Env Preflight

Inspect env key presence without printing values:

```bash
for host in $LICHEN_MAINNET_VPS_HOSTS; do
  echo "== $host env-mainnet keys =="
  ssh -p 2222 "ubuntu@$host" \
    "sudo awk -F= '/^[A-Z0-9_]+=/{print \$1}' /etc/lichen/env-mainnet | sort"
  echo "== $host custody-env-mainnet keys =="
  ssh -p 2222 "ubuntu@$host" \
    "sudo awk -F= '/^[A-Z0-9_]+=/{print \$1}' /etc/lichen/custody-env-mainnet | sort"
done
```

Required validator env keys:

```text
LICHEN_NETWORK
LICHEN_RPC_PORT
LICHEN_WS_PORT
LICHEN_P2P_PORT
LICHEN_EXTERNAL_ADDR
LICHEN_KEYPAIR_PASSWORD
LICHEN_SIGNER_BIND
LICHEN_SIGNER_AUTH_TOKEN
LICHEN_CONTRACTS_DIR
RUST_LOG
LICHEN_INCIDENT_STATUS_FILE
LICHEN_SIGNED_METADATA_MANIFEST_FILE
LICHEN_SERVICE_FLEET_CONFIG_FILE
LICHEN_SERVICE_FLEET_STATUS_FILE
LICHEN_EXTRA_ARGS
CUSTODY_URL
CUSTODY_API_AUTH_TOKEN
```

Required validator extra args:

```text
LICHEN_EXTRA_ARGS=--auto-update=off --archive-mode --cold-store /var/lib/lichen/archive-mainnet
```

Public RPC validators are archive validators. Do not launch or roll a public
RPC node with state-only storage: consensus state can remain valid while
`getTransactionsByAddress`, wallet activity, explorer activity, and historical
block/transaction lookups lose their backed source rows. `deploy/setup.sh`
creates `/var/lib/lichen/archive-<network>` and the systemd unit expands
`$LICHEN_EXTRA_ARGS` so the three validator flags above arrive as separate argv
entries.

Required base custody env keys:

```text
CUSTODY_DB_PATH
CUSTODY_API_AUTH_TOKEN
CUSTODY_MASTER_SEED_FILE
CUSTODY_DEPOSIT_MASTER_SEED_FILE
CUSTODY_LICHEN_RPC_URL
CUSTODY_POLL_INTERVAL_SECS
CUSTODY_DEPOSIT_TTL_SECS
CUSTODY_LISTEN_PORT
CUSTODY_SIGNER_AUTH_TOKEN
LICHEN_KEYPAIR_PASSWORD
LICHEN_INCIDENT_STATUS_FILE
RUST_LOG
CUSTODY_TREASURY_KEYPAIR
CUSTODY_REQUIRED_ROUTES
```

Mainnet deposit and auth TTL policy:

- `CUSTODY_DEPOSIT_TTL_SECS=86400`: issued deposit addresses expire after 24
  hours if unfunded.
- bridge access auth max TTL is 24 hours; helper default is 600 seconds unless
  `--ttl-secs` is passed.
- withdrawal access auth max TTL is 24 hours; helper default is 600 seconds.
- set `CUSTODY_WITHDRAWAL_PENDING_BURN_TTL_SECS=3600` for mainnet launch so a
  created withdrawal expires after 1 hour if the burn is not submitted. The
  binary default is longer, so make the launch value explicit.

## Phase 4: Custody Route Pre-Staging

Create the route profile on the operator machine first. Keep filled copies out
of git.

```bash
cp deploy/custody-env-mainnet.example /tmp/custody-routes-mainnet.env.template
```

The production route profile should be installed as:

```text
/etc/lichen/custody-routes-mainnet.env
```

Recommended mainnet template:

```bash
# Route policy.
CUSTODY_REQUIRED_ROUTES=solana,ethereum,bnb,neox,bitcoin

# Mainnet operational TTLs.
CUSTODY_DEPOSIT_TTL_SECS=86400
CUSTODY_WITHDRAWAL_PENDING_BURN_TTL_SECS=3600

# Solana mainnet.
CUSTODY_SOLANA_RPC_URL=REPLACE_WITH_PRIVATE_SOLANA_MAINNET_RPC_URL
CUSTODY_SOLANA_CONFIRMATIONS=32
CUSTODY_SOLANA_FEE_PAYER=/etc/lichen/secrets/solana-fee-payer-mainnet.json
CUSTODY_SOLANA_USDC_MINT=EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
CUSTODY_SOLANA_USDT_MINT=Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB

# Ethereum mainnet.
CUSTODY_ETH_RPC_URL=REPLACE_WITH_PRIVATE_ETHEREUM_MAINNET_RPC_URL
CUSTODY_ETH_CHAIN_ID=1
CUSTODY_EVM_CONFIRMATIONS=12
CUSTODY_ETH_USDC_TOKEN_ADDR=0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48
CUSTODY_ETH_USDT_TOKEN_ADDR=0xdAC17F958D2ee523a2206206994597C13D831ec7

# BNB Smart Chain mainnet. Requires Binance-Peg asset approval.
CUSTODY_BNB_RPC_URL=REPLACE_WITH_PRIVATE_BSC_MAINNET_RPC_URL
CUSTODY_BNB_CHAIN_ID=56
CUSTODY_BSC_USDC_TOKEN_ADDR=0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d
CUSTODY_BSC_USDT_TOKEN_ADDR=0x55d398326f99059fF775485246999027B3197955

# Neo X mainnet. This is EVM/0x-format, not Neo N3 N-address format.
CUSTODY_NEOX_RPC_URL=REPLACE_WITH_PRIVATE_NEO_X_MAINNET_RPC_URL
CUSTODY_NEOX_CHAIN_ID=47763
CUSTODY_NEOX_CONFIRMATIONS=12
CUSTODY_NEOX_NEO_TOKEN_ADDR=REPLACE_WITH_APPROVED_NEO_X_NEO_CONTRACT

# Bitcoin mainnet.
CUSTODY_BTC_RPC_URL=REPLACE_WITH_PRIVATE_BITCOIN_MAINNET_RPC_URL
CUSTODY_BTC_RPC_USER=REPLACE_WITH_BITCOIN_RPC_USER
CUSTODY_BTC_RPC_PASSWORD=REPLACE_WITH_BITCOIN_RPC_PASSWORD
CUSTODY_BTC_NETWORK=mainnet
CUSTODY_BTC_CONFIRMATIONS=6
CUSTODY_BTC_FEE_RATE_SATS_VB=5
CUSTODY_TREASURY_BTC=REPLACE_WITH_BTC_TREASURY_ADDRESS
```

If a source-chain custody route is not approved, remove it from
`CUSTODY_REQUIRED_ROUTES` and keep it hidden from public wallet surfaces. Do not
leave a route in `CUSTODY_REQUIRED_ROUTES` with placeholder values. This does
not remove the genesis-native wrapped token contract, DEX pair, AMM pool, or
router route.

Install the filled profile on the genesis/US VPS first:

```bash
scp -P 2222 /secure/local/custody-routes-mainnet.env ubuntu@15.204.229.189:~/custody-routes-mainnet.env
ssh -p 2222 ubuntu@15.204.229.189 '
  sudo install -m 600 -o root -g root ~/custody-routes-mainnet.env /etc/lichen/custody-routes-mainnet.env
  rm ~/custody-routes-mainnet.env
'
```

Install the same profile on EU and SEA after the US profile passes non-wrapped
route verification.

Before genesis, run source-route verification without `--require-wrapped`:

```bash
ssh -p 2222 ubuntu@15.204.229.189 '
  cd ~/lichen
  sudo bash scripts/apply-custody-route-profile.sh \
    --profile /etc/lichen/custody-routes-mainnet.env \
    --target /etc/lichen/custody-env-mainnet \
    --routes "$(sudo awk -F= "/^CUSTODY_REQUIRED_ROUTES=/{print \$2}" /etc/lichen/custody-routes-mainnet.env | tail -1)"
  sudo bash scripts/verify-custody-routes.sh \
    --env-file /etc/lichen/custody-env-mainnet \
    --routes "$(sudo awk -F= "/^CUSTODY_REQUIRED_ROUTES=/{print \$2}" /etc/lichen/custody-routes-mainnet.env | tail -1)"
'
```

Do not use `--require-wrapped` before genesis because the Lichen-side wrapped
contract addresses are generated/synced after genesis.

## Phase 5: Keys, Treasuries, And Funding

### Validator Keys

Each validator host must have its own validator keypair. Do not share validator
keypairs across hosts.

Generate and record public keys during the genesis prep. The clean-slate script
does this automatically; the manual path must capture the same evidence:

```bash
sudo python3 -c "import json; print(json.load(open('/var/lib/lichen/state-mainnet/validator-keypair.json'))['publicKeyBase58'])"
```

Record only public keys in the launch log.

### Release And Metadata Signing Key

The release-signing public address expected by clients is:

```text
8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk
```

Only the public address belongs in runbooks, signed metadata, and launch
evidence. If the signer is rotated, regenerate the signed metadata manifest and
release signatures from the new key before publishing the release.

The private signing key is staged only long enough to generate signed metadata
manifests and release artifact signatures, then removed or unmounted.
Never generate a replacement signing key on a VPS.

### Custody Seeds

Custody seeds are service secrets, not validator chain state. They may be
distributed only to hosts intentionally running custody:

```text
/etc/lichen/secrets/custody-master-seed-mainnet.txt
/etc/lichen/secrets/custody-deposit-seed-mainnet.txt
```

Permissions:

```bash
sudo chown root:lichen /etc/lichen/secrets/custody-*-seed-mainnet.txt
sudo chmod 640 /etc/lichen/secrets/custody-*-seed-mainnet.txt
```

Use the same custody seeds on all custody hosts that need to derive the same
deposit and treasury paths. Do not distribute genesis wallet material with the
custody seed bundle.

### Treasury Choice

For mainnet, prefer explicit treasury addresses controlled by the approved
operator threshold stack:

```bash
CUSTODY_TREASURY_SOLANA=REPLACE_WITH_SOLANA_TREASURY_ADDRESS
CUSTODY_TREASURY_ETH=REPLACE_WITH_ETHEREUM_TREASURY_ADDRESS
CUSTODY_TREASURY_BNB=REPLACE_WITH_BSC_TREASURY_ADDRESS
CUSTODY_TREASURY_NEOX=REPLACE_WITH_NEO_X_TREASURY_ADDRESS
```

If these are unset, custody derives deterministic treasury addresses from the
custody master seed. That can work technically, but for mainnet it must be an
explicit operator decision with backup, recovery, and threshold controls
documented before funding.

Neo X treasury addresses are EVM `0x...` addresses. This is correct for Neo X.
It is not the same address format as Neo N3.

### Solana Fee Payer

Create the Solana fee-payer keypair on a secure operator machine or HSM-backed
workflow, not in the repo.

Install it on every custody host:

```bash
scp -P 2222 /secure/local/solana-fee-payer-mainnet.json ubuntu@<HOST>:~/solana-fee-payer-mainnet.json
ssh -p 2222 ubuntu@<HOST> '
  sudo install -m 640 -o root -g lichen \
    ~/solana-fee-payer-mainnet.json \
    /etc/lichen/secrets/solana-fee-payer-mainnet.json
  rm ~/solana-fee-payer-mainnet.json
'
```

Fund it with SOL before enabling Solana deposits/withdrawals. It pays for ATA
creation and Solana sweep/release transactions. Record only the public address
and funding transaction hash.

### Source-Chain Gas Funding

Before public custody:

- Solana fee payer has enough SOL.
- Solana treasury token accounts exist or can be created.
- Ethereum treasury has ETH for gas.
- BSC treasury has BNB for gas.
- Neo X treasury has GAS for gas.
- Stablecoin source treasuries hold only dust-test liquidity until the route
  has passed dust deposit and withdrawal.

## Phase 6: Genesis Price File

Mainnet genesis refuses stale compiled defaults. Use either a reviewed
`--genesis-prices-file` or live provider fetch. The preferred launch path is an
audited file committed to the launch packet, not to the repo.

Example schema:

```json
{
  "licn_usd_8dec": 10000000,
  "wsol_usd_8dec": 0,
  "weth_usd_8dec": 0,
  "wbnb_usd_8dec": 0,
  "wneo_usd_8dec": 0,
  "wgas_usd_8dec": 0,
  "wbtc_usd_8dec": 0
}
```

Replace every `0` with the captured USD price multiplied by `100000000` and
rounded to the nearest integer. Include all six fields. Record:

- data source
- capture timestamp
- raw API response hash or archived response
- reviewer signoff

Do not proceed if prices cannot be sourced and reviewed.

## Phase 7: Mainnet Genesis

This is the manual gated shape. It mirrors the clean-slate script but keeps
custody stopped until the chain is rolling.

### 7.1 Stop Mainnet Services

On all hosts:

```bash
sudo systemctl stop lichen-custody-mainnet 2>/dev/null || true
sudo systemctl stop lichen-validator-mainnet 2>/dev/null || true
```

Confirm no mainnet process remains:

```bash
pgrep -af 'lichen-(validator|custody).*mainnet' || true
```

### 7.2 Confirm Empty Mainnet State

On all hosts:

```bash
sudo test ! -e /var/lib/lichen/state-mainnet || sudo find /var/lib/lichen/state-mainnet -maxdepth 1 -mindepth 1 -print
sudo test ! -e /var/lib/lichen/custody-db-mainnet || sudo find /var/lib/lichen/custody-db-mainnet -maxdepth 1 -mindepth 1 -print
```

If anything exists, stop and classify it. Do not delete mainnet state without an
explicit owner-approved reset string recorded in the launch log.

### 7.3 Prepare Validator Keypairs

Create or confirm validator keypairs on all four hosts without starting the
validator service:

```bash
export STATE=/var/lib/lichen/state-mainnet
export KP_PASS="$(sudo awk -F= '/^LICHEN_KEYPAIR_PASSWORD=/{print substr($0, index($0,$2))}' /etc/lichen/env-mainnet)"

sudo mkdir -p "$STATE"
sudo chown lichen:lichen "$STATE"
if [ ! -f "$STATE/validator-keypair.json" ]; then
  sudo -u lichen env LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
    /usr/local/bin/lichen identity new \
    --output "$STATE/validator-keypair.json"
fi

sudo python3 -c "import json; print(json.load(open('/var/lib/lichen/state-mainnet/validator-keypair.json'))['publicKeyBase58'])"
```

Before genesis, the only expected file in `/var/lib/lichen/state-mainnet` is
`validator-keypair.json` plus an optional staged `seeds.json`. If any RocksDB,
WAL, block, or partial genesis files appear, stop and classify them before
continuing. Never guess which key was embedded.

### 7.4 Prepare Wallet Artifacts On US Seed

On the US seed:

```bash
export NET=mainnet
export STATE=/var/lib/lichen/state-mainnet
export KP_PASS="$(sudo awk -F= '/^LICHEN_KEYPAIR_PASSWORD=/{print substr($0, index($0,$2))}' /etc/lichen/env-mainnet)"

sudo -u lichen env \
  HOME=/var/lib/lichen \
  LICHEN_HOME=/var/lib/lichen \
  LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
  LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
  ./scripts/generate-genesis.sh \
    --prepare-wallet \
    --network "$NET" \
    --output-dir "$STATE"
```

The wallet artifacts stay on the genesis host only.

### 7.5 Create Genesis On US Seed

Use the four planned validator pubkeys for bridge and oracle committees. Only
the US seed is the initial slot-zero consensus validator.

```bash
export NET=mainnet
export STATE=/var/lib/lichen/state-mainnet
export PRICE_FILE=/secure/launch/genesis-prices-mainnet.json
export SEED_VALIDATOR_PUBKEY=REPLACE_WITH_US_VALIDATOR_PUBKEY
export EU_VALIDATOR_PUBKEY=REPLACE_WITH_EU_VALIDATOR_PUBKEY
export SEA_VALIDATOR_PUBKEY=REPLACE_WITH_SEA_VALIDATOR_PUBKEY
export IN_VALIDATOR_PUBKEY=REPLACE_WITH_IN_VALIDATOR_PUBKEY
export KP_PASS="$(sudo awk -F= '/^LICHEN_KEYPAIR_PASSWORD=/{print substr($0, index($0,$2))}' /etc/lichen/env-mainnet)"

sudo -u lichen env \
  HOME=/var/lib/lichen \
  LICHEN_HOME=/var/lib/lichen \
  LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
  LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
  ./scripts/generate-genesis.sh \
    --network "$NET" \
    --db-path "$STATE" \
    --wallet-file "$STATE/genesis-wallet.json" \
    --initial-validator "$SEED_VALIDATOR_PUBKEY" \
    --bridge-validator "$SEED_VALIDATOR_PUBKEY" \
    --bridge-validator "$EU_VALIDATOR_PUBKEY" \
    --bridge-validator "$SEA_VALIDATOR_PUBKEY" \
    --bridge-validator "$IN_VALIDATOR_PUBKEY" \
    --oracle-operator "$SEED_VALIDATOR_PUBKEY" \
    --oracle-operator "$EU_VALIDATOR_PUBKEY" \
    --oracle-operator "$SEA_VALIDATOR_PUBKEY" \
    --oracle-operator "$IN_VALIDATOR_PUBKEY" \
	    --genesis-prices-file "$PRICE_FILE"
```

Verify the generated consensus timing before distributing `genesis.json`.
All validators must run the same values; do not leave the old multi-second BFT
timeouts on a 400ms network.

```bash
sudo jq '.consensus | {
  slot_duration_ms,
  propose_timeout_base_ms,
  prevote_timeout_base_ms,
  precommit_timeout_base_ms,
  max_phase_timeout_ms
}' "$STATE/genesis.json"
```

Expected values:

```json
{
  "slot_duration_ms": 400,
  "propose_timeout_base_ms": 800,
  "prevote_timeout_base_ms": 500,
  "precommit_timeout_base_ms": 500,
  "max_phase_timeout_ms": 5000
}
```

Record:

- genesis hash
- genesis pubkey
- validator pubkeys
- price file hash
- exact command shape without secret values

### 7.6 Start US Seed

```bash
sudo systemctl start lichen-validator-mainnet
sleep 10
curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'
```

US seed must produce blocks before joiners start.

### 7.7 Start Joiners From Empty State

On EU, SEA, and IN:

```bash
sudo test -f /var/lib/lichen/state-mainnet/seeds.json || sudo install -m 644 -o lichen -g lichen /etc/lichen/seeds.json /var/lib/lichen/state-mainnet/seeds.json
sudo systemctl start lichen-validator-mainnet
```

Then wait for sync:

```bash
for i in $(seq 1 60); do
  curl -fsS http://127.0.0.1:9899 \
    -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' && break
  sleep 5
done
```

Joiners must sync from peers. They must not receive US RocksDB state or genesis
wallet files.

## Phase 8: Chain Verification

On every host:

```bash
curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'

curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'

curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getLichenBridgeStats","params":[]}'

curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getLichenOracleStats","params":[]}'
```

Public edge check:

```bash
curl -fsS https://rpc.lichen.network \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
```

Required result:

- all four validators report healthy
- all four advance slots
- public RPC reports healthy and fresh block age
- bridge committee count and threshold are correct
- oracle committee/feed count is correct
- no host shows a different genesis hash or stalled tip

## Phase 9: Post-Genesis Bootstrap

On the US seed:

```bash
cd ~/lichen
sudo bash scripts/vps-post-genesis.sh mainnet
```

Stage the offline release-signing key temporarily and generate signed metadata:

```bash
cd ~/lichen
SIGNED_METADATA_KEYPAIR=/secure/offline-mounted/release-signing-keypair-mainnet.json \
DEPLOY_NETWORK=mainnet \
./scripts/first-boot-deploy.sh \
  --rpc http://127.0.0.1:9899 \
  --skip-build

sudo install -m 640 -o root -g lichen \
  ~/lichen/signed-metadata-manifest-mainnet.json \
  /etc/lichen/signed-metadata-manifest-mainnet.json
```

Remove or unmount the staged signing key immediately after use.

Verify governed signer custody before any launch cleanup:

```bash
cd ~/lichen
LICHEN_KEYPAIR_PASSWORD=REPLACE_WITH_KEYPAIR_PASSWORD \
scripts/verify-governed-key-custody.sh \
  --rpc-url http://127.0.0.1:9899 \
  --keys-dir /secure/offline-mounted/genesis-keys
```

Required result: every live `getGenesisAccounts` governed signer role resolves
to a loadable encrypted keypair in the private/offline bundle. This gate is
required for `community_treasury`, `builder_grants`, `reserve_pool`,
`validator_rewards`, `founding_symbionts`, `ecosystem_partnerships`, and the
genesis primary signer. Do not proceed with cleanup, rotation, or large governed
transfers if any live signer key is missing. Governed signer rotation is itself
a governed operation and requires the currently configured threshold; it cannot
be performed later by the genesis primary key alone.

The June 2026 testnet used a one-time, chain-id-gated
`testnet_governed_signer_recovery_v1` activation after signer custody was lost.
That hook must never be used as the mainnet custody plan. Mainnet launch approval
requires this verifier to pass before cleanup, plus two independently restored
private/offline backups of the same governed signer bundle.

Verify manifest signer:

```bash
curl -fsS http://127.0.0.1:9899 \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' \
  | python3 -c '
import json, sys
d=json.load(sys.stdin)["result"]
print(d["signer"])
assert d["signer"] == "8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk"
'
```

Distribute service secrets only after chain verification passes:

- custody seeds
- custody runtime env
- custody route profile
- Solana fee payer if the host runs Solana custody
- signed metadata manifest
- custody treasury keypair
- release-signing key only if absolutely required for that host's bootstrap,
  then remove it

Do not distribute validator chain state or genesis wallet material.

## Phase 10: Wrapped Contract Pin Sync

After genesis and signed metadata install, refresh Lichen-side custody pins on
each custody host:

```bash
cd ~/lichen
sudo bash scripts/sync-custody-wrapped-contracts.sh \
  --env-file /etc/lichen/custody-env-mainnet \
  --rpc-url http://127.0.0.1:9899
```

Then require the full route verifier:

```bash
sudo bash scripts/verify-custody-routes.sh \
  --env-file /etc/lichen/custody-env-mainnet \
  --routes "$(sudo awk -F= '/^CUSTODY_REQUIRED_ROUTES=/{print $2}' /etc/lichen/custody-env-mainnet | tail -1)" \
  --require-wrapped
```

This must pass before custody starts.

## Phase 11: Custody Start

Start custody only after:

- all four validators are healthy
- post-genesis bootstrap passed
- wrapped contract pins are synced
- source route verifier passes with `--require-wrapped`
- source treasuries and gas wallets are funded for dust tests
- public wallet routes are still hidden

On each custody host:

```bash
sudo systemctl start lichen-custody-mainnet
sleep 5
curl -fsS http://127.0.0.1:9106/health
sudo systemctl status lichen-custody-mainnet --no-pager
```

Authenticated status check without printing the token:

```bash
TOKEN="$(sudo awk -F= '/^CUSTODY_API_AUTH_TOKEN=/{print substr($0, index($0,$2))}' /etc/lichen/custody-env-mainnet)"
curl -fsS http://127.0.0.1:9106/status \
  -H "Authorization: Bearer $TOKEN" \
  | python3 -m json.tool
unset TOKEN
```

Required status:

- health is `ok`
- no permanent failed sweep jobs
- no permanent failed credit jobs
- no unexpected pending withdrawals
- signer threshold matches launch policy
- route configs are visible as enabled only for intended routes

## Phase 12: Dust Deposit Tests

Run one route at a time. Use tiny amounts. Do not fund large liquidity before
the route passes both directions.

For each source route:

1. Create a bridge auth payload.
2. Create a deposit.
3. Send a dust source-chain deposit.
4. Wait for source confirmations.
5. Confirm custody sees the deposit.
6. Confirm sweep to source treasury.
7. Confirm Lichen credit/mint.
8. Confirm source deposit address no longer holds the swept asset.
9. Confirm custody `/status` has no new permanent failure.
10. Record deposit ID, source tx, sweep tx, Lichen credit tx, and timestamps.

Example Lichen deposit creation through public RPC:

```bash
cargo run -p lichen-rpc --bin bridge_auth_payload -- \
  --chain solana \
  --asset usdc \
  --seed-byte 42 \
  --ttl-secs 600 \
  > /tmp/bridge-auth-solana-usdc.json

curl -fsS https://rpc.lichen.network \
  -H 'Content-Type: application/json' \
  --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"createBridgeDeposit\",\"params\":[$(cat /tmp/bridge-auth-solana-usdc.json)]}"
```

Day-one route test order:

1. Neo X `gas` -> `wGAS`, only if Neo X GAS route is approved.
2. Neo X `neo` -> `wNEO`, only if official NEO token route is approved.
3. Ethereum `eth` -> `wETH`.
4. Ethereum `usdc`/`usdt` -> `lUSD`.
5. Solana `sol` -> `wSOL`.
6. Solana `usdc`/`usdt` -> `lUSD`.
7. BNB `bnb` -> `wBNB`, only after BNB route approval.
8. BSC Binance-Peg `usdc`/`usdt` -> `lUSD`, only after Binance-Peg approval.

If any route fails, pause only that route. Do not reset the chain to fix a
custody route.

## Phase 13: Dust Withdrawal Tests

For each route that passed deposits:

1. Create a signed withdrawal authorization.
2. Create the custody withdrawal job.
3. Burn the exact wrapped amount on Lichen.
4. Submit the burn signature to custody.
5. Wait for custody burn verification.
6. Wait for source-chain broadcast.
7. Wait for destination confirmation.
8. Confirm custody marks the job `confirmed`.
9. Confirm destination wallet received the funds.
10. Record job ID, burn tx, outbound tx, destination address, and timings.

Example helper flow:

```bash
cargo run -p lichen-rpc --bin keypair_from_seed_byte -- \
  --seed-byte 81 \
  --output /tmp/lichen-withdraw-user.json

cargo run -p lichen-rpc --bin withdrawal_auth_payload -- \
  --asset wsol \
  --amount 1000000 \
  --dest-chain solana \
  --dest-address REPLACE_WITH_SOLANA_DESTINATION \
  --seed-byte 81 \
  --ttl-secs 600 \
  > /tmp/withdrawal-auth-wsol.json

cargo run -p lichen-rpc --bin wrapped_burn -- \
  --rpc-url https://rpc.lichen.network \
  --contract "$CUSTODY_WSOL_TOKEN_ADDR" \
  --amount 1000000 \
  --seed-byte 81
```

Route-specific rules:

- `wNEO` withdrawals must be exact whole NEO lots:
  `amount % 1000000000 == 0`.
- `wGAS` uses Neo X EVM `0x...` destination addresses.
- `wETH`, `wBNB`, and Neo X routes use EVM `0x...` destination addresses.
- `wSOL` uses a Solana base58 destination address.
- `lUSD` withdrawals must choose a configured source stablecoin route:
  `preferred_stablecoin=usdc` or `preferred_stablecoin=usdt`.

## Phase 14: Public Route Enablement

Only after a route passes deposit and withdrawal:

- enable that route in wallet/frontend config
- publish the route as supported
- keep low caps
- watch custody `/status`, route events, source RPC failures, reserve ledger,
  source treasury balances, and public RPC health

Routes that did not pass remain hidden, even if their env values are staged.

## Phase 15: Monitoring And Watchtower

Start or confirm monitoring for:

- validator service state
- local RPC health
- public RPC health
- slot/block age
- bridge stats
- oracle stats
- custody `/health`
- authenticated custody `/status`
- sweep backlog
- credit backlog
- withdrawal backlog
- source RPC error rate
- source treasury gas balances
- Solana fee payer SOL balance
- reserve ledger deltas
- bridge route pause/restriction status
- signed metadata signer and manifest hash

Recommended watchtower controls:

```bash
export LICHEN_WATCHTOWER_ROUTE_HEALTH_TARGETS='[
  {"chain":"solana","asset":"sol"},
  {"chain":"solana","asset":"usdc"},
  {"chain":"ethereum","asset":"eth"},
  {"chain":"ethereum","asset":"usdc"},
  {"chain":"bnb","asset":"bnb"},
  {"chain":"neox","asset":"gas"}
]'
export LICHEN_WATCHTOWER_CUSTODY_STATUS_URL=http://127.0.0.1:9106/status
export LICHEN_WATCHTOWER_CUSTODY_BACKLOG_WARNING=10
export LICHEN_WATCHTOWER_CUSTODY_BACKLOG_CRITICAL=50
```

Add `wNEO` and BSC stablecoin targets only after their route gates pass.

## Phase 16: Incident Controls

If custody misbehaves while chain consensus is healthy:

1. Hide affected route in public surfaces.
2. Pause/restrict the affected bridge route if available.
3. Stop `lichen-custody-mainnet` if funds are at risk.
4. Preserve custody DB and logs.
5. Do not reset validator state.
6. Diagnose source RPC, gas, signer, treasury, route config, and wrapped-token
   pins.
7. Restart custody only after the failing route has a written fix and a repeat
   dust test plan.

If validator consensus diverges or public RPC reports inconsistent state:

1. Stop all validators.
2. Preserve state and logs.
3. Do not copy state between hosts.
4. Do not reset mainnet without a separate owner/governance incident decision.
5. Fix through a signed release or explicit recovery plan.

## Launch Log Template

```markdown
# Lichen Mainnet Launch Log

Date:
Operators:
Release tag:
Release commit:
Release URL:
CI run URL:
Release signer:
SHA256SUMS hash:

## Hosts

| Host | IP | Arch | Validator hash | Custody hash | Status |
| --- | --- | --- | --- | --- | --- |

## External Route Decisions

| Route | Public day one? | RPC provider | Token/mint | Treasury | Gas funded | Approval |
| --- | --- | --- | --- | --- | --- | --- |

## Validator Pubkeys

US:
EU:
SEA:

## Genesis

Genesis time:
Genesis hash:
Genesis pubkey:
Price file hash:
Bridge validators:
Oracle operators:

## Post-Genesis

Signed metadata signer:
Manifest hash:
Wrapped pins synced:
Route verifier result:

## Custody

Custody started at:
Health:
Status summary:
Signer threshold:

## Dust Tests

| Route | Deposit tx | Sweep tx | Credit tx | Burn tx | Outbound tx | Result |
| --- | --- | --- | --- | --- | --- | --- |

## Public Enablement

Routes enabled:
Caps:
Monitoring:
On-call:
```

## Final Go/No-Go Checklist

Before public launch:

- [ ] CI is green for the exact release commit.
- [ ] Release is signed and attached assets verify.
- [ ] All four VPSes run the signed release binaries.
- [ ] Mainnet state was empty before genesis.
- [ ] Genesis hash is recorded.
- [ ] All four validators are healthy and advancing.
- [ ] Public RPC is healthy.
- [ ] Signed metadata signer matches the trust anchor.
- [ ] Custody route profile is installed and secret-safe.
- [ ] Solana fee payer is installed and funded.
- [ ] Source treasuries have gas.
- [ ] Wrapped token pins are synced.
- [ ] Full custody verifier passes for every public route.
- [ ] Custody health and authenticated status pass.
- [ ] Each public route passed dust deposit.
- [ ] Each public route passed dust withdrawal.
- [ ] Wallet/frontend enables only passed routes.
- [ ] Monitoring and on-call are active.
- [ ] The launch log has no secrets.
