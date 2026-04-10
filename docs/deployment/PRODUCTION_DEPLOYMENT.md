# Lichen Deployment Runbook

This is the current operator runbook for the repo as it exists today.

Use this document as the canonical workflow for:

- local validator development via `scripts/start-local-3validators.sh`
- VPS validator deployment via `deploy/setup.sh`
- local full-stack extension when custody, faucet, and browser flows are needed
- genesis DB creation and post-genesis bootstrap
- signed release and signed metadata generation
- local ZK proof generation

This runbook intentionally prefers the scripts that are verified in the current tree over older narrative docs.

## Supported operator paths

| Workflow | Supported entrypoint |
| --- | --- |
| Local validator development | `scripts/start-local-3validators.sh` |
| VPS validator deployment | `deploy/setup.sh` |

`deploy/setup.sh` is now responsible for the public edge as well: it installs the checked-in Caddy config from `deploy/Caddyfile.*`, enables the `caddy` service, uses internal TLS for Cloudflare-origin traffic, and keeps raw RPC, WebSocket, faucet, and custody ports off the public firewall surface.

## TLS termination model

Production RPC and WebSocket listeners do not terminate TLS inside the Rust services.
The supported production shape is:

- Cloudflare or another trusted edge in front of the node
- Caddy on the origin host terminating HTTPS and WSS with the checked-in `deploy/Caddyfile.*` configs
- local origin proxying from Caddy to the raw app listeners on `127.0.0.1`
- firewall rules that keep raw RPC and WS ports off the public internet

Current checked-in origin mappings are:

- mainnet: `rpc.lichen.network -> 127.0.0.1:9899`, `ws.lichen.network -> 127.0.0.1:9900`
- testnet: `testnet-rpc.lichen.network -> 127.0.0.1:8899`, `testnet-ws.lichen.network -> 127.0.0.1:8900`

Operational rule: direct exposure of the raw RPC or WebSocket listeners is unsupported for production. If Caddy or the firewall posture is missing, the node is not in a production-ready network shape even if the Rust services are running.

## Supporting scripts

| Task | Supporting script |
| --- | --- |
| Local full stack extension | `scripts/start-local-stack.sh` |
| Local full-stack stop/status | `scripts/stop-local-stack.sh`, `scripts/status-local-stack.sh` |
| Genesis wallet + DB creation | `scripts/generate-genesis.sh` |
| Post-genesis bootstrap | `scripts/first-boot-deploy.sh` |
| VPS post-genesis keypair copy | `scripts/vps-post-genesis.sh` |
| Manual single-node debugging | `lichen-start.sh` |
| Release signing | `scripts/generate-release-keys.sh`, `scripts/sign-release.sh` |
| Signed metadata manifest | `scripts/generate-signed-metadata-manifest.js` |
| ZK proof generation | `target/release/zk-prove` |

Important distinctions:

- `run-validator.sh` is a local-development helper behind the 3-validator launcher, not a supported operator entrypoint on its own.
- `scripts/start-local-stack.sh` extends the supported local validator path with custody, faucet, and post-genesis bootstrap.
- `scripts/first-boot-deploy.sh` is post-genesis bootstrap only. Genesis itself deploys the contract catalog.
- `lichen-start.sh` is for manual foreground debugging only, not part of the supported operator runbook.

## Network selection and public ingress

- Local browser workflows default to `local-testnet`.
- Production portals default to `mainnet` and call `https://rpc.lichen.network` unless the operator or user explicitly selects `testnet`.
- Public testnet RPC lives at `https://testnet-rpc.lichen.network`.
- A healthy testnet redeploy does not make the production portals healthy if `rpc.lichen.network` or its Cloudflare/Caddy/origin path is down.
- Browser CORS, incident-status, signed-metadata, and symbol-registry errors on the portals can be caused by an ingress failure first. A Cloudflare `502` is surfaced by browsers as a CORS-style failure because the error page is not the RPC app.

## Prerequisites

Install these before running any of the workflows below:

- Rust toolchain with `wasm32-unknown-unknown`
- Node.js for signed metadata manifest generation
- Python 3 with `venv` support for the repo Python tools
- `curl` and `python3`
- On VPSes, a Rust install that can be loaded in non-login shells via `. "$HOME/.cargo/env"`

Deployment helper note:

- `scripts/first-boot-deploy.sh` now bootstraps `.venv` from `sdk/python/requirements.txt` if the required Python modules are missing, but it still requires `python3 -m venv` to work.

Recommended setup from repo root:

```bash
rustup target add wasm32-unknown-unknown
python3 -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt 2>/dev/null || true
cargo build --release --bin lichen-validator --bin lichen-genesis --bin lichen-faucet --bin lichen-custody --bin lichen --bin zk-prove
./scripts/build-all-contracts.sh
```

If you want the repo-wide convenience build instead:

```bash
make build
```

## Paths and outputs

Know the path conventions before you start:

| Environment | Validator state |
| --- | --- |
| Local validator 1 | `data/state-7001` on testnet, `data/state-8001` on mainnet |
| Local validator 2 | `data/state-7002` on testnet, `data/state-8002` on mainnet |
| Local validator 3 | `data/state-7003` on testnet, `data/state-8003` on mainnet |
| VPS / systemd | `/var/lib/lichen/state-testnet` or `/var/lib/lichen/state-mainnet` |

Other important outputs:

- local signed metadata manifest: `signed-metadata-manifest-testnet.json` or `signed-metadata-manifest-mainnet.json`
- local full-stack logs: `/tmp/lichen-local-testnet` or `/tmp/lichen-local-mainnet`
- deploy manifest: `deploy-manifest.json`
- VPS validator envs: `/etc/lichen/env-testnet` and `/etc/lichen/env-mainnet`
- VPS custody envs: `/etc/lichen/custody-env` and `/etc/lichen/custody-env-mainnet`

## Keypair password policy

Outside explicit local development, set `LICHEN_KEYPAIR_PASSWORD` before the first validator, genesis, custody, or signer start and keep the same value available on every restart.

Operational rules:

- canonical validator, treasury, genesis-primary, and signer keypair JSON files are encrypted at rest when `LICHEN_KEYPAIR_PASSWORD` is set
- production loaders now refuse plaintext keypair files unless `LICHEN_LOCAL_DEV=1` or `LICHEN_ALLOW_PLAINTEXT_KEYPAIRS=1` is explicitly set
- local launchers still allow plaintext compatibility for throwaway development, but the secure local E2E path should export `LICHEN_KEYPAIR_PASSWORD` so the same code path is exercised before redeploy
- any helper copy of a keypair file must preserve owner-only permissions; use the checked-in scripts rather than ad hoc `cp`

## Local Runbook

### Local 3-validator cluster

Use this when you want the verified multi-validator path with signed metadata generation.

Start from a clean state:

```bash
export LICHEN_KEYPAIR_PASSWORD='local-e2e-secret'
./scripts/start-local-3validators.sh start-reset
```

Reuse an existing cluster without resetting state:

```bash
./scripts/start-local-3validators.sh start
```

Check health:

```bash
./scripts/start-local-3validators.sh status
curl -s http://127.0.0.1:8899 -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
curl -s http://127.0.0.1:8901 -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
curl -s http://127.0.0.1:8903 -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
```

Stop the cluster:

```bash
./scripts/start-local-3validators.sh stop
```

What this launcher does:

- starts 3 validators through `run-validator.sh`
- creates local genesis state on validator 1 when needed
- writes validator state under `data/state-7001`, `data/state-7002`, `data/state-7003`
- generates a signed metadata manifest once the cluster is healthy

What it does not do:

- it does not start custody or faucet
- it does not manage local stack status outside validator health

### Local full stack

Use this when you want validators plus custody, faucet, and first-boot deploy.

Start:

```bash
export LICHEN_KEYPAIR_PASSWORD='local-e2e-secret'
./scripts/start-local-stack.sh testnet
```

Status:

```bash
./scripts/status-local-stack.sh testnet
```

Stop:

```bash
./scripts/stop-local-stack.sh testnet
```

What the full-stack launcher starts:

- 3 local validators
- custody service
- faucet service on testnet
- `scripts/first-boot-deploy.sh` after validator health

Where to look for logs:

```bash
ls /tmp/lichen-local-testnet
```

### Local verification checklist

After starting either local flow, verify the chain before moving on:

```bash
bash tests/local-multi-validator-test.sh
curl -s http://127.0.0.1:8899/api/v1/pairs | python3 -m json.tool
ls -l signed-metadata-manifest-testnet.json
```

If you want to reuse an already-running cluster in the validator test harness:

```bash
LICHEN_REUSE_EXISTING_CLUSTER=1 bash tests/local-multi-validator-test.sh
```

Keep `LICHEN_KEYPAIR_PASSWORD` exported while running Python or SDK-driven E2Es against that cluster. The helper files under `keypairs/` and `data/state-*/genesis-keys/` may now be encrypted canonical keypair JSON, and the SDK loader uses the same password to open them.

## Genesis Runbook

Use `scripts/generate-genesis.sh` instead of hand-building a `genesis.json` file.

### Step 1: prepare wallet artifacts

Example for testnet:

```bash
./scripts/generate-genesis.sh \
  --network testnet \
  --prepare-wallet \
  --output-dir ./artifacts/testnet
```

This writes the wallet artifacts used for the next step, including `genesis-wallet.json`.

### Step 2: create the genesis DB

Example for local validator 1:

```bash
./scripts/generate-genesis.sh \
  --network testnet \
  --db-path ./data/state-7001 \
  --wallet-file ./artifacts/testnet/genesis-wallet.json \
  --validator-keypair ./data/state-7001/validator-keypair.json
```

Equivalent mainnet example:

```bash
./scripts/generate-genesis.sh \
  --network mainnet \
  --db-path ./data/state-8001 \
  --wallet-file ./artifacts/mainnet/genesis-wallet.json \
  --validator-keypair ./data/state-8001/validator-keypair.json
```

Important rules:

- local state directories are keyed by P2P port, not by network name
- VPS systemd state directories are keyed by network name
- use `--validator-keypair` or explicit `--initial-validator` inputs; the wrapper rejects the legacy handwritten flow

## Contract deployment and post-genesis bootstrap

Genesis auto-deploys the canonical contract catalog. LICN is native and is not part of that deployed contract set.

After a supported local or VPS genesis node is healthy, the canonical post-genesis bootstrap step is `scripts/first-boot-deploy.sh`.

Run it manually against a healthy validator when needed:

```bash
./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:8899 --skip-build
```

Use `http://127.0.0.1:9899` for mainnet, or set `DEPLOY_NETWORK=mainnet` explicitly when bootstrapping against a mainnet RPC.

Use it without `--skip-build` if WASM artifacts are missing:

```bash
./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:8899
```

What it does:

- waits for validator health
- rebuilds `deploy-manifest.json` from the live genesis-deployed symbol registry
- keeps local helper key material aligned with the genesis admin key
- generates a signed metadata manifest when Node.js and the release-signing key are present

For VPSes, run `scripts/vps-post-genesis.sh` after genesis creation to copy the genesis admin and faucet keypairs into the system paths expected by custody and faucet before you start those services.

Outputs to verify:

- `deploy-manifest.json`
- signed metadata manifest file
- healthy DEX pair list from `/api/v1/pairs`

Operational notes:

- On VPSes, run `scripts/first-boot-deploy.sh` from the operator-owned repo checkout, not from the `lichen` service account, unless the repo path is traversable by that account.
- The script now refuses to trust a stale `deploy-manifest.json` unless it matches the live symbol registry. Older copies of the repo can still carry stale manifests, so a clean-slate redeploy should treat that file as disposable.
- If the script generates the signed metadata file in the repo checkout, install it into `/etc/lichen/signed-metadata-manifest-<net>.json` before relying on public browser flows.

Local convenience wrapper:

```bash
make deploy-local
```

## VPS Runbook

### Step 1: stage the repo correctly

Do not use `git archive` for VPS staging in this repo. Ignored directories in this tree still matter for deployment.

Use a full repo sync into `~/lichen` instead:

```bash
rsync -az --delete \
  --exclude '.git' \
  --exclude 'target' \
  --exclude 'compiler/target' \
  --exclude 'data' \
  --exclude 'logs' \
  --exclude 'node_modules' \
  --exclude 'dist' \
  ./ <host>:~/lichen/
```

For a clean-slate redeploy, also remove stale repo-generated artifacts on the host before first boot:

```bash
rm -f ~/lichen/deploy-manifest.json
rm -f ~/lichen/signed-metadata-manifest-testnet.json ~/lichen/signed-metadata-manifest-mainnet.json
```

### Step 2: build release binaries on the host

If the host was updated via `rsync` hotfixes, force Cargo to see fresh source mtimes before rebuilding. `rsync -a` preserves timestamps, and stale remote artifact mtimes can otherwise cause `cargo build` to reuse old validator binaries even when the new Rust source is present.

From the staged repo:

```bash
find . \
  \( -path './target' -o -path './compiler/target' -o -path './node_modules' \) -prune -o \
  -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) -exec touch {} +
```

From the staged repo:

```bash
. "$HOME/.cargo/env"
cargo build --release --bin lichen-validator --bin lichen-genesis --bin lichen-faucet --bin lichen-custody --bin lichen --bin zk-prove
./scripts/build-all-contracts.sh
```

Critical contract artifact invariant:

- Genesis replay reads the top-level tracked files `contracts/<name>/<name>.wasm`.
- `./scripts/build-all-contracts.sh` rewrites those top-level files.
- Never rebuild contracts on only the genesis host after staging the repo to joining validators. That changes deterministic program addresses and guarantees a genesis state-root mismatch.
- Choose one path per rollout and keep it consistent across every validator:
  - stage one identical prebuilt repo bundle to every host and do not rebuild contracts on any host, or
  - run `./scripts/build-all-contracts.sh` on every host before `deploy/setup.sh`.
- `deploy/setup.sh` now prints the installed top-level contract bundle hash. Verify that hash matches on every VPS before creating genesis or starting joining validators.

Example verification command:

```bash
command find /var/lib/lichen/contracts -maxdepth 2 -name '*.wasm' | sort | xargs shasum -a 256 | shasum -a 256
```

Why the explicit `cargo` env load matters:

- `ssh <host> 'cd ~/lichen && cargo build ...'` uses a non-login shell on many VPSes, and `cargo` will be missing from `PATH` unless you source `~/.cargo/env` yourself.

### Step 3: install services and env files

On each VPS:

```bash
sudo bash deploy/setup.sh testnet
```

Or for mainnet:

```bash
sudo bash deploy/setup.sh mainnet
```

This creates:

- system user `lichen`
- `/etc/lichen`
- `/var/lib/lichen`
- `/var/log/lichen`
- `/etc/lichen/env-testnet` or `/etc/lichen/env-mainnet`
- validator, custody, and faucet systemd units

Service names:

- `lichen-validator-testnet`
- `lichen-validator-mainnet`
- `lichen-custody`
- `lichen-custody-mainnet`
- `lichen-faucet` on testnet only

### Step 4: bootstrap the genesis VPS

Run these steps on the first validator only.

`deploy/setup.sh` auto-generates `LICHEN_KEYPAIR_PASSWORD` in `/etc/lichen/env-<net>` and writes the same password into `/etc/lichen/custody-env[-mainnet]`. The validator, genesis builder, custody service, and threshold signer share the canonical encrypted keypair format, so the same password must be present anywhere those files are loaded.

To inspect or export your validator keypair at any time:

```bash
# Load the password
LICHEN_KEYPAIR_PASSWORD=$(grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-testnet | cut -d= -f2-)
export LICHEN_KEYPAIR_PASSWORD

# Show public key and EVM address
lichen identity export --keypair /var/lib/lichen/state-testnet/validator-keypair.json

# Also reveal the private seed (handle with extreme care)
lichen identity export --keypair /var/lib/lichen/state-testnet/validator-keypair.json --reveal-seed
```

1. Start the validator once so it generates `validator-keypair.json`.
2. Record `publicKeyBase58` from that file.
3. Stop the service and clear any temporary state.
4. Prepare wallet artifacts.
5. Create the genesis DB.
6. Start the validator again.

Concrete sequence for testnet:

```bash
sudo systemctl start lichen-validator-testnet
sudo install -D -m 600 -o lichen -g lichen \
  /var/lib/lichen/state-testnet/validator-keypair.json \
  /var/lib/lichen/validator-keypair-testnet.json
sudo python3 -c "import json; print(json.load(open('/var/lib/lichen/state-testnet/validator-keypair.json'))['publicKeyBase58'])"
sudo systemctl stop lichen-validator-testnet
sudo rm -rf /var/lib/lichen/state-testnet
sudo rm -rf /var/lib/lichen/.lichen
sudo install -d -m 750 -o lichen -g lichen /var/lib/lichen/state-testnet
sudo install -m 600 -o lichen -g lichen \
  /var/lib/lichen/validator-keypair-testnet.json \
  /var/lib/lichen/state-testnet/validator-keypair.json

sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  lichen-genesis --network testnet --prepare-wallet --output-dir /var/lib/lichen/genesis-keys-testnet

PRICE_JSON=$(curl -sf 'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]')
export GENESIS_SOL_USD=$(echo "$PRICE_JSON" | python3 -c "import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='SOLUSDT']")
export GENESIS_ETH_USD=$(echo "$PRICE_JSON" | python3 -c "import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='ETHUSDT']")
export GENESIS_BNB_USD=$(echo "$PRICE_JSON" | python3 -c "import sys,json; [print(t['price']) for t in json.load(sys.stdin) if t['symbol']=='BNBUSDT']")

sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  GENESIS_SOL_USD=$GENESIS_SOL_USD GENESIS_ETH_USD=$GENESIS_ETH_USD GENESIS_BNB_USD=$GENESIS_BNB_USD \
  lichen-genesis --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --wallet-file /var/lib/lichen/genesis-keys-testnet/genesis-wallet.json \
  --initial-validator <VALIDATOR_PUBKEY>

sudo systemctl start lichen-validator-testnet
```

Preserve the generated `validator-keypair.json` across the state wipe. If you delete it and restart the service on a different keypair than the one baked into genesis, the node will come up at stake `0`, stay at slot `0`, and never produce the founding blocks.

If `api.binance.com` is blocked or returns `451` on the host, export `GENESIS_SOL_USD`, `GENESIS_ETH_USD`, and `GENESIS_BNB_USD` from a trusted fallback source before running `lichen-genesis`.

### Step 5: run post-genesis deploy on the genesis VPS (MANDATORY)

After the validator is healthy, run the post-genesis deploy from the repo checkout on the genesis host as the checkout owner. **This step is required** — without it, the signed metadata manifest is not generated and all frontend portals (DEX, wallet, explorer, etc.) will fail to resolve contract addresses.

```bash
cd ~/lichen
DEPLOY_NETWORK=testnet ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:8899 --skip-build
```

For mainnet, run:

```bash
cd ~/lichen
DEPLOY_NETWORK=mainnet ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:9899 --skip-build
```

This is the cleanest way to refresh the deploy manifest, helper key alignment, and signed metadata manifest in the current repo state.

The helper key alignment step now copies the encrypted `genesis-primary-*.json` file with mode `600`. That repo-local helper represents the current wrapped-token operational minter key used by local bootstrap flows, not the long-lived governed admin authority.

If the configured `LICHEN_SIGNED_METADATA_KEYPAIR_FILE` lives under `/etc/lichen/secrets/`, the checkout owner may not be able to read it directly. In that case, copy it to a temporary user-readable path and pass it explicitly:

```bash
sudo cp /etc/lichen/secrets/release-signing-keypair-testnet.json ~/release-signing-keypair-testnet.json
sudo chown "$USER":"$USER" ~/release-signing-keypair-testnet.json
chmod 600 ~/release-signing-keypair-testnet.json
cd ~/lichen
SIGNED_METADATA_KEYPAIR=$HOME/release-signing-keypair-testnet.json \
  DEPLOY_NETWORK=testnet ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:8899 --skip-build
rm -f ~/release-signing-keypair-testnet.json
```

If the script generated the signed metadata file under `~/lichen/`, install it into the RPC-configured path before continuing. **Do not skip this step — the DEX and all frontends depend on it:**

```bash
sudo install -m 640 -o root -g lichen \
  ~/lichen/signed-metadata-manifest-testnet.json \
  /etc/lichen/signed-metadata-manifest-testnet.json
```

Verify the manifest is served correctly:

```bash
curl -s http://127.0.0.1:8899 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' \
  | python3 -c "import sys,json; d=json.load(sys.stdin); p=json.loads(d['result']['envelope']['payload']); print(f'{len(p.get(\"symbol_registry\",[]))} symbols in manifest')"
```

If this does not return the expected symbol count (28 for genesis), check that the manifest file exists at the path configured by `LICHEN_SIGNED_METADATA_MANIFEST_FILE` in `/etc/lichen/env-<net>` and restart the validator.

### Step 6: join additional validators

On every joining VPS, verify the installed state seed file matches the staged release file:

```bash
cmp /var/lib/lichen/state-testnet/seeds.json /etc/lichen/seeds.json
```

For mainnet, compare `/var/lib/lichen/state-mainnet/seeds.json` instead.

`deploy/setup.sh` installs the checked-in `seeds.json` to both `/etc/lichen/seeds.json` and `/var/lib/lichen/state-<net>/seeds.json`. Joining validators use that seed file directly; no bootstrap flags or env overrides are required.

Copy the signed metadata manifest from the genesis VPS to each joining VPS, so all nodes serve the same contract address data to frontends:

```bash
# From genesis VPS:
scp -P 2222 /etc/lichen/signed-metadata-manifest-testnet.json ubuntu@<JOINING_VPS>:~/
# On joining VPS:
sudo install -m 640 -o root -g lichen \
  ~/signed-metadata-manifest-testnet.json \
  /etc/lichen/signed-metadata-manifest-testnet.json
rm ~/signed-metadata-manifest-testnet.json
```

Then start the service:

```bash
sudo systemctl start lichen-validator-testnet
```

### Step 7: custody and faucet

`deploy/setup.sh` creates the custody env files, but you still need to provision the secret material.

Run `scripts/vps-post-genesis.sh` after genesis creation so `/etc/lichen/custody-treasury-<net>.json` is populated from the encrypted `genesis-primary-*.json` artifact with secure permissions. Despite the historical path name, this file is now the wrapped-token operational minter key used by custody for `mint()` flows; wrapped-token admin and contract ownership move to governance during genesis.

Provision before starting custody:

- `/etc/lichen/secrets/custody-master-seed-testnet.txt`
- `/etc/lichen/secrets/custody-deposit-seed-testnet.txt`

Or the mainnet equivalents.

Permission model for those files matters:

- `/etc/lichen/secrets` must be `root:lichen` with mode `750`.
- Seed files must be `root:lichen` with mode `640`.
- `/etc/lichen/custody-treasury-<net>.json` must remain `lichen:lichen` with mode `600`.
- `LICHEN_KEYPAIR_PASSWORD` must be present in `/etc/lichen/custody-env` or `/etc/lichen/custody-env-mainnet` before custody starts, because that service now loads the same canonical encrypted keypair JSON used by genesis and validator helpers.
- If you provision them as `root:root 600`, `lichen-custody` will fail with `Permission denied`.

Wrapped-token authority split after genesis:

- contract owner and wrapped-token admin live under the governance authority
- custody keeps the current wrapped-token minter key until governance executes `set_minter`
- wrapped-token attester rotation now runs through the oracle-committee approval lane via `set_attester`
- cold admin transfer remains on the governance root via `transfer_admin` and `accept_admin`

Then start the services:

```bash
sudo systemctl start lichen-custody
sudo systemctl start lichen-faucet
```

For mainnet, start custody only:

```bash
sudo systemctl start lichen-custody-mainnet
```

Mainnet uses `lichen-custody-mainnet` and has no faucet.

### Step 8: external ingress and browser smoke tests

Do not stop after internal `127.0.0.1` health checks. Validate the public path that browsers will actually use.

For public testnet:

```bash
curl -si -X OPTIONS https://testnet-rpc.lichen.network/ \
  -H 'Origin: https://dex.lichen.network' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: content-type'

curl -s https://testnet-rpc.lichen.network/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'

curl -s https://testnet-rpc.lichen.network/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getIncidentStatus","params":[]}'

curl -s https://testnet-rpc.lichen.network/ -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}'
```

If production portals are expected to stay usable, run the same checks against `https://rpc.lichen.network` too. Production portals default to `mainnet`, so a dead mainnet origin will surface as frontend CORS, incident-status, signed-metadata, and missing-contract-address errors even while `testnet-rpc.lichen.network` is healthy.

### Step 9: day-2 operations

Useful commands:

```bash
sudo systemctl status lichen-validator-testnet
sudo journalctl -u lichen-validator-testnet -n 200 --no-pager
sudo systemctl status lichen-custody
sudo journalctl -u lichen-custody -n 200 --no-pager
curl -s http://127.0.0.1:8899 -X POST -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
```

Firewall minimums:

- HTTP ingress: `80/tcp`
- HTTPS ingress: `443/tcp`
- testnet P2P: `7001/tcp`
- mainnet P2P: `8001/tcp`

Expose RPC, WS, faucet, and custody only through the reverse proxy layout you actually operate. The supported repo-managed layout lives in `deploy/Caddyfile.common`, `deploy/Caddyfile.testnet`, `deploy/Caddyfile.testnet-us`, `deploy/Caddyfile.mainnet`, and `deploy/Caddyfile.mainnet-us`, uses internal TLS at the VPS edge for Cloudflare-origin traffic, and is installed by `deploy/setup.sh`.

### Step 10: backup, restore, and disaster recovery

Do not use `reset-blockchain.sh` on VPS hosts. That script is intentionally limited to local and developer reset flows and is not the supported production restore path.

The authoritative backup set for a VPS validator is:

- `/etc/lichen/env-<net>`
- `/etc/lichen/custody-env` on testnet or `/etc/lichen/custody-env-mainnet` on mainnet
- `/etc/lichen/secrets/`
- `/etc/lichen/custody-treasury-<net>.json` (wrapped-token operational minter key)
- `/etc/lichen/signed-metadata-manifest-<net>.json`
- `/etc/lichen/incident-status-<net>.json`
- `/etc/lichen/service-fleet-<net>.json`
- `/etc/lichen/key-hierarchy.md`
- `/etc/lichen/drill-register.md`
- `/var/lib/lichen/state-<net>`
- `/var/lib/lichen/.lichen`
- `/var/lib/lichen/service-fleet-status-<net>.json`
- `/var/lib/lichen/custody-db` on testnet or `/var/lib/lichen/custody-db-mainnet` on mainnet
- testnet only: `/var/lib/lichen/faucet-keypair-testnet.json` and `/var/lib/lichen/airdrops.json`

Identity-critical files live inside that set. In particular, do not lose `/var/lib/lichen/state-<net>/validator-keypair.json` or `/var/lib/lichen/.lichen/node_identity.json`, or the node will come back with a different validator or P2P identity.

`deploy/setup.sh` can recreate `/var/lib/lichen/contracts` and the checked-in systemd units from the correct repo release, but include them in the backup if you want a faster single-archive restore and an easier offline drill.

Create an offline snapshot with services stopped. Start from the repo release that is actually running on the host.

```bash
NET=testnet
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
BACKUP_DIR=/var/backups/lichen/$NET-$STAMP
VALIDATOR_SERVICE=lichen-validator-$NET
CUSTODY_SERVICE=lichen-custody
CUSTODY_ENV=/etc/lichen/custody-env
CUSTODY_DB=/var/lib/lichen/custody-db
OPTIONAL_SERVICE=lichen-faucet
OPTIONAL_PATHS=(
  /var/lib/lichen/faucet-keypair-testnet.json
  /var/lib/lichen/airdrops.json
)

if [ "$NET" = "mainnet" ]; then
  CUSTODY_SERVICE=lichen-custody-mainnet
  CUSTODY_ENV=/etc/lichen/custody-env-mainnet
  CUSTODY_DB=/var/lib/lichen/custody-db-mainnet
  OPTIONAL_SERVICE=
  OPTIONAL_PATHS=()
fi

sudo install -d -m 750 -o root -g root "$BACKUP_DIR"
if [ -n "$OPTIONAL_SERVICE" ]; then
  sudo systemctl stop "$OPTIONAL_SERVICE"
fi
sudo systemctl stop "$CUSTODY_SERVICE"
sudo systemctl stop "$VALIDATOR_SERVICE"

sudo tar --xattrs --acls --numeric-owner -cpf "$BACKUP_DIR/lichen-$NET.tar" \
  /etc/lichen/env-$NET \
  "$CUSTODY_ENV" \
  /etc/lichen/secrets \
  /etc/lichen/custody-treasury-$NET.json \
  /etc/lichen/signed-metadata-manifest-$NET.json \
  /etc/lichen/incident-status-$NET.json \
  /etc/lichen/service-fleet-$NET.json \
  /etc/lichen/key-hierarchy.md \
  /etc/lichen/drill-register.md \
  /var/lib/lichen/state-$NET \
  /var/lib/lichen/.lichen \
  /var/lib/lichen/service-fleet-status-$NET.json \
  "$CUSTODY_DB" \
  /var/lib/lichen/contracts \
  "${OPTIONAL_PATHS[@]}"

(cd "$BACKUP_DIR" && sha256sum "lichen-$NET.tar" > SHA256SUMS)

sudo systemctl start "$VALIDATOR_SERVICE"
sudo systemctl start "$CUSTODY_SERVICE"
if [ -n "$OPTIONAL_SERVICE" ]; then
  sudo systemctl start "$OPTIONAL_SERVICE"
fi
```

Record the archive path, `SHA256SUMS`, the repo revision used to create the backup, and the contract bundle hash from Step 2 in the deployed `/etc/lichen/drill-register.md` before moving the archive to offline storage.

Restore onto a clean or rebuilt VPS by re-establishing the supported filesystem layout first, then extracting the preserved state back in place. A restore is not a genesis rebuild. Do not wipe the recovered state and do not rerun `lichen-genesis` when you are restoring an existing validator.

```bash
NET=testnet
BACKUP_DIR=/var/backups/lichen/testnet-20260406T120000Z
VALIDATOR_SERVICE=lichen-validator-$NET
CUSTODY_SERVICE=lichen-custody
RPC_PORT=8899
OPTIONAL_SERVICE=lichen-faucet

if [ "$NET" = "mainnet" ]; then
  CUSTODY_SERVICE=lichen-custody-mainnet
  RPC_PORT=9899
  OPTIONAL_SERVICE=
fi

cd ~/lichen
sudo bash deploy/setup.sh "$NET"

if [ -n "$OPTIONAL_SERVICE" ]; then
  sudo systemctl stop "$OPTIONAL_SERVICE"
fi
sudo systemctl stop "$CUSTODY_SERVICE"
sudo systemctl stop "$VALIDATOR_SERVICE"

(cd "$BACKUP_DIR" && sha256sum -c SHA256SUMS)
sudo tar --xattrs --acls --numeric-owner -xpf "$BACKUP_DIR/lichen-$NET.tar" -C /

sudo chown -R lichen:lichen /var/lib/lichen
sudo chown root:lichen /etc/lichen/secrets
sudo chmod 750 /etc/lichen/secrets
sudo find /etc/lichen/secrets -type f -exec chown root:lichen {} \;
sudo find /etc/lichen/secrets -type f -exec chmod 640 {} \;

sudo systemctl daemon-reload
sudo systemctl start "$VALIDATOR_SERVICE"
sudo systemctl start "$CUSTODY_SERVICE"
if [ -n "$OPTIONAL_SERVICE" ]; then
  sudo systemctl start "$OPTIONAL_SERVICE"
fi

curl -s http://127.0.0.1:$RPC_PORT -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'

curl -s http://127.0.0.1:$RPC_PORT -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getIncidentStatus","params":[]}'

curl -s http://127.0.0.1:$RPC_PORT -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}'
```

If the restore archive did not include `/etc/lichen/custody-treasury-<net>.json` or the faucet keypair, but `/var/lib/lichen/state-<net>/genesis-keys` was restored, repopulate those service-facing files before restarting custody or faucet:

```bash
cd ~/lichen
sudo bash scripts/vps-post-genesis.sh "$NET" --no-restart
```

After the node is healthy, run the same public smoke tests from Step 8 and update the deployed `/etc/lichen/drill-register.md` with the restore transcript, checksum verification, recovered file inventory, and owner signoff. The quarterly offline backup restore drill in `docs/deployment/ROTATION_AND_RESTORE_DRILLS.md` should use this exact sequence.

## Manual single-node debugging

Use `lichen-start.sh` only when you need a manual, foreground, or one-off debugging flow instead of the supported local launcher or VPS systemd path.

Examples:

```bash
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-start.sh testnet --foreground
mkdir -p ./data/state-mainnet
cp ./seeds.json ./data/state-mainnet/seeds.json
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-start.sh mainnet
```

Notes:

- `--custody` is restricted to explicit local development
- this script is useful for manual debugging, but not the canonical steady-state service model

## Release signing and signed metadata

### Generate an offline release signing key

```bash
./scripts/generate-release-keys.sh ./offline-release
```

This prints the trusted signer address. Embed that address in `validator/src/updater.rs` before relying on signed release verification.

Keep the generated keypair offline.

### Sign a release artifact set

```bash
sha256sum <files...> > SHA256SUMS
./scripts/sign-release.sh SHA256SUMS ./offline-release/release-signing-keypair.json
```

This writes `SHA256SUMS.sig` next to `SHA256SUMS`.

### Generate a signed metadata manifest

Local example:

```bash
export SIGNED_METADATA_KEYPAIR=/secure/local-signing/release-signing-keypair.json

node scripts/generate-signed-metadata-manifest.js \
  --rpc http://127.0.0.1:8899 \
  --network local-testnet \
  --keypair "$SIGNED_METADATA_KEYPAIR" \
  --out ./signed-metadata-manifest-testnet.json
```

VPS example:

```bash
node scripts/generate-signed-metadata-manifest.js \
  --rpc http://127.0.0.1:8899 \
  --network testnet \
  --keypair /secure/offline-mounted/release-signing-keypair.json \
  --out /etc/lichen/signed-metadata-manifest-testnet.json
```

The local 3-validator launcher generates this manifest automatically.

On VPS deploys, `first-boot-deploy.sh` must now regenerate the manifest, install it into the configured `/etc/lichen/` target, and verify that the validator serves the expected DEX-related symbol registry entries back through `getSignedMetadataManifest`. If Node.js, the release-signing keypair, or the install step is missing, the deploy fails instead of continuing half-configured.

`first-boot-deploy.sh` no longer assumes a repo-local signing key. Set `LICHEN_SIGNED_METADATA_KEYPAIR_FILE` in `/etc/lichen/env-<net>` via `deploy/setup.sh`, or export `SIGNED_METADATA_KEYPAIR` explicitly for local-only workflows.

## ZK proof generation

Build the proof CLI:

```bash
cargo build --release -p lichen-cli --bin zk-prove
```

Generate proofs:

```bash
./target/release/zk-prove shield --amount 1000000000
./target/release/zk-prove unshield --amount 1000000000 --merkle-root <hex> --recipient <hex> --blinding <hex> --serial <hex>
./target/release/zk-prove transfer --transfer-json ./transfer-witness.json
```

The proof CLI writes JSON to stdout. That JSON is the input for the transaction-building side of the shielded flow.

Verifier note:

- Lichen uses transparent STARK proofs
- there is no separate trusted setup ceremony to ship to operators
- the verifier lives in the validator runtime; operators do not manually distribute a static trusted verifier key bundle as part of the normal deployment path

## Frontend and portal deployment

Static portals deploy through Cloudflare Pages. Current project names use the `lichen-network-` prefix.

Important behavior:

- In production, the portals default to `mainnet` until the user selects another network.
- A successful testnet rollout only makes the portals work if either the user switches to `testnet` or `rpc.lichen.network` is also healthy.

| Portal | Project name | Directory |
| --- | --- | --- |
| website | `lichen-network-website` | `website/` |
| explorer | `lichen-network-explorer` | `explorer/` |
| wallet | `lichen-network-wallet` | `wallet/` |
| dex | `lichen-network-dex` | `dex/` |
| marketplace | `lichen-network-marketplace` | `marketplace/` |
| programs | `lichen-network-programs` | `programs/` |
| developers | `lichen-network-developers` | `developers/` |
| monitoring | `lichen-network-monitoring` | `monitoring/` |
| faucet | `lichen-network-faucet` | `faucet/` |

Supported repo deploy command:

```bash
./scripts/deploy-cloudflare-pages.sh <portal>
```

The wrapper runs the frontend asset audit, stages the selected portal into a clean temp directory, verifies required staged assets such as the DEX TradingView bundle, and then calls Wrangler from that staged `--cwd`.

Raw CLI pattern for reference only:

```bash
npx wrangler pages deploy <dir> --project-name lichen-network-<portal> --commit-dirty=true
```

Do not use the raw command as the normal repo workflow. It can silently omit git-ignored runtime assets that the staged deploy wrapper preserves.

`faucet-service` is the VPS/API backend. The browser faucet portal in `faucet/` is a separate Cloudflare Pages project: `lichen-network-faucet`.

## Final operator checklist

Before you call a deployment complete, verify all of the following:

- validator health returns `ok`
- expected contracts are present
- signed metadata manifest exists and is current
- local or VPS DEX pair list is populated
- custody and faucet health endpoints respond if those services are enabled
- external CORS preflight and JSON-RPC checks succeed for every public RPC hostname you expect browsers to use
- `getIncidentStatus` and `getSignedMetadataManifest` succeed through the public edge, not just on `127.0.0.1`
- release artifacts are signed if you are cutting an upgrade
- `zk-prove` builds and runs if you are validating privacy flows

Useful checks:

```bash
curl -s http://127.0.0.1:8899/api/v1/pairs | python3 -m json.tool
curl -s http://127.0.0.1:9105/health | python3 -m json.tool
curl -s http://127.0.0.1:9100/health | python3 -m json.tool
ls -l deploy-manifest.json signed-metadata-manifest-testnet.json
```

If this document and the scripts ever disagree, trust the scripts first and then update this runbook immediately.
