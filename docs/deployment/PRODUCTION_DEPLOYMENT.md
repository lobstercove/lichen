# Lichen Deployment Runbook

This is the current operator runbook for the repo as it exists today.

Use this document as the canonical workflow for:

- local validator development via `scripts/start-local-3validators.sh`
- local production-parity stack validation via `scripts/start-local-stack.sh`
- VPS validator deployment via `deploy/setup.sh`
- local full-stack extension when custody, faucet, and browser flows are needed
- genesis DB creation and post-genesis bootstrap
- signed release and signed metadata generation
- local ZK proof generation

This runbook intentionally prefers the scripts that are verified in the current tree over older narrative docs.

Mandatory state/sync policy: [TESTNET_STATE_AND_SYNC_POLICY.md](TESTNET_STATE_AND_SYNC_POLICY.md). For any shared testnet, staging, or mainnet-like network, do not reset state and do not distribute a copied validator state directory unless the network owner explicitly approves that exact reset. Joining validators must sync from their own state directories.

Restriction schema activation policy: [RESTRICTION_SCHEMA_ACTIVATION.md](RESTRICTION_SCHEMA_ACTIVATION.md). RG-804 activation is testnet-only, uses `scripts/activate-restriction-schema-testnet.sh`, requires explicit owner approval for that exact activation, stops validators only long enough to set the shipped state-root schema flag, and records per-host sync evidence. It is not a reset path and must not copy chain state.

## Supported operator paths

| Workflow | Supported entrypoint |
| --- | --- |
| **VPS rolling signed-release update** | Non-destructive default for code-only upgrades: `LICHEN_RELEASE_TAG=vX.Y.Z scripts/rolling-release-deploy.sh testnet` |
| **VPS clean-slate redeploy** | Owner-approved only: `LICHEN_OWNER_APPROVED_RESET='owner-approved:testnet:15.204.229.189,37.59.97.61,15.235.142.253' LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM='clean-slate:testnet:15.204.229.189,37.59.97.61,15.235.142.253' scripts/clean-slate-redeploy.sh testnet` |
| **Testnet restriction schema activation** | Owner-approved only: `LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION='owner-approved:restriction-schema:testnet:15.204.229.189,37.59.97.61,15.235.142.253' LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM='activate-restriction-schema:testnet:15.204.229.189,37.59.97.61,15.235.142.253' scripts/activate-restriction-schema-testnet.sh` |
| Local validator development | `scripts/start-local-3validators.sh` |
| **Local production-parity stack** | `scripts/start-local-stack.sh testnet` |
| VPS initial provisioning | `deploy/setup.sh` |

`deploy/setup.sh` is now responsible for the public edge as well: it installs the checked-in Caddy config from `deploy/Caddyfile.*`, enables the `caddy` service, uses internal TLS for Cloudflare-origin traffic, and keeps raw RPC, WebSocket, faucet, and custody ports off the public firewall surface.

## Deployment path selection

Choose the least destructive path that matches the evidence:

| Situation | Required path | State policy |
| --- | --- | --- |
| Code-only validator, RPC, P2P, indexing, or performance fix | `LICHEN_RELEASE_TAG=vX.Y.Z scripts/rolling-release-deploy.sh <testnet|mainnet>` | Do not reset or copy state. Install the signed release and restart one validator at a time. |
| One node is stale because of local disk/log pressure, bad service state, or a host rebuild | Fix disk/log pressure first, then rejoin only that node from its own preserved validator identity | Do not wipe the whole network. Do not copy RocksDB state from another validator. Delete that node's local `state-<net>` only when the operator explicitly approves that single-node rejoin. |
| Genesis contents must change, launch rehearsal must start from block 0, or every node has provably inconsistent chain state | Owner-approved `scripts/clean-slate-redeploy.sh <testnet|mainnet>` | Destructive. Stop services, flush all validator state, create genesis on the seed, then start joiners from empty state so they sync from peers. |
| Contract metadata, custody, faucet, Caddy, firewall, or secret ownership issue while the chain is healthy | Fix the affected service/run `deploy/setup.sh` or `scripts/first-boot-deploy.sh` as documented | Do not touch validator state. |

Rolling-release rules:

- A rolling release must use a published or draft GitHub Release with `SHA256SUMS` and `SHA256SUMS.sig` attached.
- `scripts/rolling-release-deploy.sh` performs the VPS disk/log preflight, refuses non-live state backup directories under `/var/lib/lichen`, installs release binaries and `seeds.json`, restarts one validator at a time, proves the service PID/start timestamp changed, verifies every running validator process in the service executes the expected release binary hash, waits for local health, then checks the public RPC edge.
- Rolling release is the default for cadence, WebSocket, RPC indexing, and consensus performance fixes because those fixes do not require a new genesis.
- Any release that changes replay, block import, post-block effects, fees, staking, oracle, or validator-set handling must include deterministic-state coverage for local-observer differences, including commit-certificate subsets.
- Any release that changes fee charging or block commit batching must prove the public `getTotalBurned` value increases after a finalized fee-bearing transaction or drill. Do not accept explorer fee display alone as proof; it can be derived from the transaction while the consensus burned counter remains stale.
- If a rolling release exposes a state-root mismatch, split tip, or stalled BFT height, stop every validator service, keep state/logs intact for evidence, fix and tag the code, then use the owner-approved clean-slate path only after the fix is verified. Do not copy RocksDB state between validators to "heal" the split.
- A full reset is not a code-deploy mechanism. Use it only when the chain identity or genesis state must intentionally change, or after captured evidence proves the whole network state is unrecoverable.

## TLS termination model

Production RPC and WebSocket listeners do not terminate TLS inside the Rust services.
The supported production shape is:

- Cloudflare or another trusted edge in front of the node
- Caddy on the origin host terminating HTTPS and WSS with the checked-in `deploy/Caddyfile.*` configs
- local origin proxying from Caddy to the raw app listeners on `127.0.0.1`
- firewall rules that keep raw RPC and WS ports off the public internet

Primary public endpoints are:

- mainnet: JSON-RPC `https://rpc.lichen.network`, WebSocket `wss://rpc.lichen.network/ws`
- testnet: JSON-RPC `https://testnet-rpc.lichen.network`, WebSocket `wss://testnet-rpc.lichen.network/ws`

Current checked-in origin mappings also include dedicated WebSocket aliases:

- mainnet: `rpc.lichen.network -> 127.0.0.1:9899`, `/ws -> 127.0.0.1:9900`, `ws.lichen.network -> 127.0.0.1:9900`
- testnet: `testnet-rpc.lichen.network -> 127.0.0.1:8899`, `/ws -> 127.0.0.1:8900`, `testnet-ws.lichen.network -> 127.0.0.1:8900`

Operational rule: direct exposure of the raw RPC or WebSocket listeners is unsupported for production. If Caddy or the firewall posture is missing, the node is not in a production-ready network shape even if the Rust services are running.
When the checked-in Caddy configs use `tls internal`, every advertised HTTPS/WSS hostname must be proxied through the trusted edge. Do not publish DNS-only `A` records for `ws.lichen.network` or `testnet-ws.lichen.network` unless Caddy is configured with a public CA certificate for those hostnames and the public WSS smoke test passes.

## Supporting scripts

| Task | Supporting script |
| --- | --- |
| **Full automated VPS redeploy** | `scripts/clean-slate-redeploy.sh` |
| **Rolling signed-release VPS deploy** | `scripts/rolling-release-deploy.sh` |
| Testnet restriction state-root schema activation | `scripts/activate-restriction-schema-testnet.sh` |
| Local full stack extension | `scripts/start-local-stack.sh` |
| Local full-stack stop/status | `scripts/stop-local-stack.sh`, `scripts/status-local-stack.sh` |
| Genesis wallet + DB creation | `scripts/generate-genesis.sh` |
| Post-genesis bootstrap | `scripts/first-boot-deploy.sh` |
| VPS post-genesis keypair copy | `scripts/vps-post-genesis.sh` |
| Manual single-node debugging | `lichen-start.sh` |
| Release signing | `scripts/generate-release-keys.sh`, `scripts/sign-release.sh` |
| Signed metadata manifest | `scripts/generate-signed-metadata-manifest.js` |
| Health check | `scripts/health-check.sh` |
| Cloudflare Pages deploy | `scripts/deploy-cloudflare-pages.sh` |
| ZK proof generation | `target/release/zk-prove` |

Important distinctions:

- `run-validator.sh` is a local-development helper behind the 3-validator launcher, not a supported operator entrypoint on its own.
- `scripts/start-local-stack.sh` extends the supported local validator path with custody, faucet, and post-genesis bootstrap.
- `scripts/first-boot-deploy.sh` is post-genesis bootstrap only. Genesis itself deploys the contract catalog.
- `scripts/first-boot-deploy.sh` is also the protocol-readiness gate: it fails unless `getLichenBridgeStats` reports a live quorum and `getLichenOracleStats` reports all launch feeds.
- `lichen-start.sh` is for manual foreground debugging only, not part of the supported operator runbook.

## Local parity contract

The repo supports two different local workflows, and they are not interchangeable:

- `scripts/start-local-3validators.sh` is for validator and consensus development. It is intentionally lighter than VPS deployment.
- `scripts/start-local-stack.sh` is the supported local analog for production-like stack validation.

If you are trying to answer "will this behave like the VPS deployment except for being disposable?", use `scripts/start-local-stack.sh`, not the validator-only launcher.

Production-parity rule:

- The acceptable difference between local parity runs and VPS runs is disposability and host orchestration.
- Local parity runs may use repo-local data paths, shell-managed processes, and loopback networking.
- Local parity runs must still use the same release binaries, the same genesis flow, the same post-genesis bootstrap, the same contract artifacts, the same signed-metadata generation path, the same custody/faucet services, and the same keypair-password policy.

Current status of that contract in this repo:

| Concern | Local 3-validator | Local full stack | VPS deployment |
| --- | --- | --- | --- |
| 3-validator consensus shape | Yes | Yes | Yes |
| Release validator binary | Yes | Yes | Yes |
| Genesis + post-genesis bootstrap | Partial | Yes | Yes |
| Custody service | No | Yes | Yes |
| Faucet service | No | Yes on testnet | Yes on US testnet host |
| Signed metadata manifest refresh | Yes | Yes | Yes |
| Same encrypted-keypair path when `LICHEN_KEYPAIR_PASSWORD` is exported | Yes | Yes | Yes |
| Public ingress via Caddy / Cloudflare | No | No | Yes |
| systemd ownership of services | No | No | Yes |
| Disposable reset to clean genesis | Yes | Yes | No by default |

Operational consequence:

- Do not treat `scripts/start-local-3validators.sh` as a production-representative environment.
- Treat `scripts/start-local-stack.sh` as the required local gate for production-like E2E and matrix work.
- Treat VPS or staging deployment as the final gate for systemd, ingress, firewall, and long-lived-state behavior.
- Treat shared testnet state as developer-owned live state. A reset requires explicit owner approval and developer communication.

Workflow parity rule:

- The parity model is seed-first, not "three equivalent local validators".
- Validator 1 or seed-01 creates genesis and owns the first post-genesis bootstrap.
- Validators 2 and 3 are joiners against that already-created chain.
- Local parity work is only considered valid when it follows that same logical sequence before tests run.

Phase-by-phase equivalence:

| Phase | Local full stack | VPS clean-slate redeploy |
| --- | --- | --- |
| Reset state | Reset repo-local `data/state-*` paths before launch when a fresh chain is required | Stop services and wipe `/var/lib/lichen/*` before redeploy |
| Seed genesis | Validator 1 pre-generates all three validator identities, creates or refreshes local genesis state, and embeds the 3-key bridge/oracle operator set while only validator 1 is active at slot zero | `seed-01` pre-generates all three VPS validator identities, creates the new genesis state, and embeds the 3-key bridge/oracle operator set while only `seed-01` is active at slot zero |
| Post-genesis bootstrap | `scripts/start-local-stack.sh` waits for the genesis artifacts and then runs `scripts/first-boot-deploy.sh` on validator 1 | `scripts/clean-slate-redeploy.sh` runs `scripts/first-boot-deploy.sh` on `seed-01` after genesis |
| Joiners come online | Validators 2 and 3 keep their own keypairs, start from empty state directories, verify the canonical genesis/network identifier, then obtain all post-genesis chain state through normal sync from peers | `seed-02` and `seed-03` keep their own keypairs, receive only service configuration/secrets for their own host, verify the canonical genesis/network identifier, and sync chain state from seed peers |
| Auxiliary services | Custody and faucet start from the genesis-derived local key material | Custody and faucet start from the seed host's provisioned key material |
| Validation gate | Run E2E and matrix workloads against the local stack, then rerun without reset | Run final staging or VPS verification for systemd, ingress, firewall, and long-lived state |

Current implementation note:

- The logical workflow is now the same: seed genesis, post-genesis bootstrap on the seed, then independently syncing joiners. Do not copy RocksDB state, block-0 RocksDB bundles from another validator, `genesis-wallet.json`, `genesis-keys/`, peer cache, validator identity, or consensus WAL to make a joiner work.
- The only acceptable shared genesis artifact is the canonical network genesis descriptor/hash, not a mutable database snapshot from one validator host.
- The remaining difference is host orchestration: local starts loopback processes with shell supervision, while VPS starts systemd services across hosts and distributes only service-level secrets.
- That infrastructure difference does not replace the VPS gate for service-orchestration, ingress, or long-lived-state behavior.

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

## VPS disk and log guardrails

The May 2026 stale-US-node incident was caused by the US VPS root filesystem filling up from old non-live chain-state backups plus unbounded sudo I/O logs. RocksDB then failed writes with `No space left on device`; the US RPC kept serving a stale slot and public RPC intermittently routed users to it.

Operational rules:

- Do not keep copied RocksDB chain-state backups on `/` or under `/var/lib/lichen` after a reset. Keep only the live `/var/lib/lichen/state-<net>` directory on each validator and move any required archive off-host or onto a separately monitored volume.
- Before a rolling release, testnet reset, audited launch rehearsal, or mainnet launch, every VPS must pass the disk preflight below before any validator service starts.
- `deploy/setup.sh` installs persistent journald limits and, when sudo I/O logging already exists, installs compression and two-day tmpfiles retention for `/var/log/sudo-io`. Re-run `deploy/setup.sh` on old hosts before using them for a reset or launch.
- A node with `getHealth.result.disk.critical=true`, an HTTP 503 `stale_tip`, or a root filesystem above the critical threshold is not eligible for Cloudflare/public RPC routing.

Required preflight on every VPS, including rolling updates:

```bash
df -h /
sudo du -sh /var/lib/lichen /var/log/journal /var/log/sudo-io 2>/dev/null || true
sudo find /var/lib/lichen -maxdepth 1 -type d \
  \( -name 'state-testnet-*' -o -name 'state-mainnet-*' -o -name '*backup*' \) -print
curl -s http://127.0.0.1:<rpc-port> -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
```

If the `find` command returns non-live state backups, remove them only after confirming they are not the active `state-<net>` path and any required archive has been moved off-host. If `/var/log/sudo-io` or `/var/log/journal` is large, run `sudo journalctl --vacuum-size=512M` and `sudo systemd-tmpfiles --clean /etc/tmpfiles.d/sudo-io-retention.conf` after `deploy/setup.sh` has installed the retention files.

## Keypair password policy

Outside explicit local development, set `LICHEN_KEYPAIR_PASSWORD` before the first validator, genesis, custody, or signer start and keep the same value available on every restart.

Operational rules:

- canonical validator, treasury, genesis-primary, and signer keypair JSON files are encrypted at rest when `LICHEN_KEYPAIR_PASSWORD` is set
- production loaders now refuse plaintext keypair files unless `LICHEN_LOCAL_DEV=1` or `LICHEN_ALLOW_PLAINTEXT_KEYPAIRS=1` is explicitly set
- local launchers still allow plaintext compatibility for throwaway development, but the secure local E2E path should export `LICHEN_KEYPAIR_PASSWORD` so the same code path is exercised before redeploy
- any helper copy of a keypair file must preserve owner-only permissions; use the checked-in scripts rather than ad hoc `cp`

## Local Runbook

### Local 3-validator cluster

Use this when you want the verified multi-validator path with signed metadata generation and nothing else.

This is not the production-parity local deployment. It is a validator-focused developer workflow.

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
- it does not mirror the VPS service layout closely enough for release-signoff E2E work

### Local full stack

Use this when you want the closest supported local analog to the VPS deployment while keeping the stack disposable.

This is the local production-parity path for E2E and matrix validation.

The required local interpretation is:

- validator 1 creates genesis
- validators 2 and 3 join that chain by importing verified block-0 state, then replaying later blocks
- custody and faucet come up against that chain
- `scripts/first-boot-deploy.sh` performs the same post-genesis bootstrap that the seed VPS performs before production-like tests run

Start:

```bash
export LICHEN_KEYPAIR_PASSWORD='local-e2e-secret'
./scripts/start-local-stack.sh testnet
```

By default this launcher resets the local validator state first so the stack comes up from fresh genesis. Set `LICHEN_LOCAL_RESET_CLUSTER=0` only when you intentionally want to reuse the existing local chain.

Status:

```bash
./scripts/status-local-stack.sh testnet
```

Stop:

```bash
./scripts/stop-local-stack.sh testnet
```

What the full-stack launcher does, in order:

- starts validator 1 and establishes genesis state
- waits for the genesis treasury and deployer key material to appear
- starts custody service
- starts faucet service on testnet
- runs `scripts/first-boot-deploy.sh` against validator 1
- starts validators 2 and 3 from empty state directories with their own validator keypairs
- validators 2 and 3 fetch the canonical genesis config from validator 1's RPC and sync/replay blocks from peers

What still differs from VPS deployment:

- processes are shell-managed rather than owned by systemd
- state lives under repo-local `data/state-*` paths rather than `/var/lib/lichen/*`
- ingress is loopback-only and does not include Caddy or Cloudflare
- SSH, firewall, service-user, and origin-edge behavior are not part of the local stack
- VPS also distributes custody/faucet/signing service secrets, while external validators receive none of those service secrets

What must not differ for production-like testing:

- release binaries and contract artifacts
- genesis and first-boot bootstrap behavior
- joiner validators must not receive copied RocksDB chain state
- custody/faucet/runtime feature set
- signed metadata generation path
- encrypted keypair handling when `LICHEN_KEYPAIR_PASSWORD` is set

Where to look for logs:

```bash
ls /tmp/lichen-local-testnet
```

### Local verification checklist

After starting either local flow, verify the chain before moving on:

```bash
for port in 8899 8901 8903; do
  curl -s "http://127.0.0.1:${port}" -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' | python3 -m json.tool
done
curl -s http://127.0.0.1:9105/health | python3 -m json.tool
curl -s http://127.0.0.1:8899/api/v1/pairs | python3 -m json.tool
ls -l signed-metadata-manifest-testnet.json
```

If your local-private harness is present, you can additionally run the old validator smoke test, but treat it as destructive:

```bash
bash scripts/run-local-private-check.sh tests/local-multi-validator-test.sh -- \
  bash tests/local-multi-validator-test.sh
```

That legacy harness flushes `data/state-*` and boots its own validator sequence. Use it only as a standalone disposable validator smoke test, not as an in-place verifier for an already-running production-parity local stack.

Keep `LICHEN_KEYPAIR_PASSWORD` exported while running Python or SDK-driven E2Es against that cluster. The helper files under `keypairs/` and `data/state-*/genesis-keys/` may now be encrypted canonical keypair JSON, and the SDK loader uses the same password to open them.

For production-like validation, the minimum local gate is:

1. Run `scripts/start-local-stack.sh testnet` from a clean reset.
2. Run the intended E2E or matrix workload.
3. Re-run the same workload without resetting state to catch reused-signer, reused-faucet, and long-lived-chain issues.
4. Only after that, run the VPS or staging gate for systemd and ingress-specific behavior.

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

### Neo public beta gate

Neo public beta activation is fail-closed. Do not set `LICHEN_GENESIS_NEO_GAS_REWARDS_ENABLE=1` on a VPS, staging host, or any public testnet/mainnet genesis unless the NX-900 manifest has already passed:

```bash
node scripts/qa/check_neo_public_beta_gate.js \
  --manifest /etc/lichen/neo-public-beta-gate-testnet.json
```

When `scripts/generate-genesis.sh` sees public Neo GAS rewards genesis activation outside `LICHEN_LOCAL_DEV=1`, it requires:

```bash
export LICHEN_NEO_PUBLIC_BETA_GATE_MANIFEST=/etc/lichen/neo-public-beta-gate-testnet.json
```

The manifest must include owner/governance/security/custody/legal/deployment approvals, numeric route and rewards caps, reward funding evidence, disclosure URL and hash, rollback/monitoring metadata, and local test evidence. For fresh genesis, the manifest's `activation.fresh_genesis_env` values must exactly match the `LICHEN_GENESIS_NEO_GAS_REWARDS_*` env values used by genesis. For a running chain, use `activation.mode=post_genesis_governance` with a governed proposal id, payload hash, and timelock instead of fresh-genesis env.

Start from `docs/deployment/NEO_PUBLIC_BETA_GATE_TEMPLATE.json`. The template intentionally does not pass validation until every placeholder is replaced with real values and every approval/evidence boolean is true.

This gate does not approve deployment by itself. It only proves the approval package is complete enough for the owner to make a deployment decision.

Developer-facing route, rewards, DEX, SDK, and PQ evidence examples live in `docs/guides/NEO_DEVELOPER_INTEGRATION.md`. Keep those examples aligned with this gate: examples may prove local integration, but public rewards activation still requires this manifest.

Local parity rehearsal must force the same gate even though local launchers set `LICHEN_LOCAL_DEV=1`:

```bash
export LICHEN_NEO_PUBLIC_BETA_GATE_REQUIRED=1
export LICHEN_NEO_PUBLIC_BETA_GATE_MANIFEST=tests/artifacts/nx-900-testnet-rehearsal-YYYYMMDD/neo-public-beta-gate-testnet.LOCAL_REHEARSAL_ONLY.json
```

For the clean seed/joiner proof, run V1 first and then start V2/V3 from empty state:

```bash
./scripts/start-local-3validators.sh start-reset-seed
./scripts/start-local-3validators.sh start-joiners-from-seed-sync
```

The V1 log must show `NX-900 Neo public beta gate: PASS` before genesis DB creation. V2/V3 must join by network sync only; do not copy RocksDB, genesis wallets, consensus WALs, or seed state into joiner directories.

The VPS clean-slate deployment path uses the same wrapper. If `LICHEN_GENESIS_NEO_GAS_REWARDS_ENABLE=1` is present in `/etc/lichen/env-<network>`, the genesis step must also carry `LICHEN_NEO_PUBLIC_BETA_GATE_MANIFEST` from that env file. Missing or failing gate validation is a deployment blocker.

### Neo liquidity corridor gate

Neo Liquidity Corridor incentives are a separate `NX-950` lane. They must not be bundled into base Neo route activation, Neo GAS rewards activation, or any unrelated release. Before any public `wNEO`/`wGAS` DEX incentive campaign starts, validate the lane manifest:

```bash
node scripts/qa/check_neo_liquidity_corridor_gate.js \
  --manifest /etc/lichen/neo-liquidity-corridor-gate-testnet.json
```

Start from `docs/deployment/NEO_LIQUIDITY_CORRIDOR_GATE_TEMPLATE.json`. The template intentionally does not pass validation until every placeholder is replaced, every required approval/evidence boolean is true, and at least one approved Neo pair is enabled.

The manifest is scoped to the existing `dex_rewards` contract and the existing Neo DEX pair IDs:

- `wNEO/lUSD`: pair and pool `8`
- `wNEO/LICN`: pair and pool `9`
- `wGAS/lUSD`: pair and pool `10`
- `wGAS/LICN`: pair and pool `11`

Current campaign activation uses a governed post-genesis payload that calls `dex_rewards.configure_lp_campaign` for the approved pair IDs, with the approved rate and per-pair budget from the manifest. Fresh chains follow the same path after genesis so reset and running-chain activation use one reviewable payload format. Do not add ad hoc genesis reward-rate shortcuts.

Fresh genesis and post-genesis deployments must wire LP accounting before any campaign payload is approved:

- `dex_amm.set_rewards_address(dex_rewards)`
- `dex_rewards.set_authorized_caller(dex_amm, enabled=true)`

This wiring only lets AMM positions sync LP reward accounting. It does not activate incentives until governed campaign rates and budgets are configured. Pausing a campaign sets the affected pair rates to zero; LP fee collection, liquidity removal, and bridge/user exits must remain available.

The gate requires product/governance/security/custody/legal/market-risk/deployment approvals, per-pool TVL/volume/reward caps, a funded LICN campaign budget, whole-lot `wNEO` preservation, route-pause behavior, user-exit proof, public disclosure, watchtower coverage, and local 3-validator DEX evidence with AMM, router, rewards, candles, and rollback checks.

Passing this checker does not deploy or activate the campaign. It only proves the approval packet and evidence are complete enough for the owner to decide whether to schedule activation.

Developers should treat the Neo Liquidity Corridor as an approved-manifest extension of the existing DEX rewards path. The public developer examples in `docs/guides/NEO_DEVELOPER_INTEGRATION.md` intentionally show pair IDs, LP pending surfaces, and `dex_rewards.configure_lp_campaign` without implying that a public incentive campaign is active.

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

cd ~/lichen
sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
  ./scripts/generate-genesis.sh \
  --network testnet --prepare-wallet --output-dir /var/lib/lichen/genesis-keys-testnet

sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
  LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
  ./scripts/generate-genesis.sh \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --wallet-file /var/lib/lichen/genesis-keys-testnet/genesis-wallet.json \
  --initial-validator <VALIDATOR_PUBKEY>

sudo systemctl start lichen-validator-testnet
```

Preserve the generated `validator-keypair.json` across the state wipe. If you delete it and restart the service on a different keypair than the one baked into genesis, the node will come up at stake `0`, stay at slot `0`, and never produce the founding blocks.

`lichen-genesis` fetches live genesis market prices from Binance first, then CoinGecko. Testnet may fall back to compiled defaults if both sources are unavailable; mainnet refuses that fallback. For mainnet or an audited reset, pass `--genesis-prices-file <path>` with `licn_usd_8dec`, `wsol_usd_8dec`, `weth_usd_8dec`, and `wbnb_usd_8dec` fields, or export all three `GENESIS_SOL_USD`, `GENESIS_ETH_USD`, and `GENESIS_BNB_USD` from a trusted snapshot.

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
  | python3 -c "import sys,json; d=json.load(sys.stdin); p=d['result']['payload']; print(f'{len(p.get(\"symbol_registry\",[]))} symbols in manifest')"
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

Do not copy `genesis-wallet.json`, `genesis-keys/`, RocksDB files, `known-peers.json`, or consensus WAL to joining validators. A joining validator needs only:

- its own encrypted `validator-keypair.json`
- `/var/lib/lichen/state-<net>/seeds.json`
- `LICHEN_KEYPAIR_PASSWORD` in `/etc/lichen/env-<net>`
- optional service-only files if this host also runs custody/faucet

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

When a VPS is expected to serve public bridge intake through RPC, the validator
service env for that network must also include:

- `CUSTODY_URL=http://127.0.0.1:9105` when custody runs locally on the same VPS,
- `CUSTODY_API_AUTH_TOKEN` matching the local custody service token.

The custody env must resolve wrapped-token route contracts to the current
on-chain symbol registry after every reset, genesis rebuild, or mainnet launch.
Pinned `CUSTODY_LUSD_TOKEN_ADDR`, `CUSTODY_WSOL_TOKEN_ADDR`,
`CUSTODY_WETH_TOKEN_ADDR`, and `CUSTODY_WBNB_TOKEN_ADDR` values must be updated
or removed before custody starts; stale addresses from a previous chain will
make `createBridgeDeposit` fail after route restrictions are lifted. Validate
the bridge route through the public RPC and each direct VPS RPC:

```bash
lichen --rpc-url https://testnet-rpc.lichen.network restriction status bridge-route solana sol
```

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

python3 - <<'PY'
import subprocess
payload = '{"jsonrpc":"2.0","id":1,"method":"subscribeSlots","params":null}\n'
for i in range(1, 11):
    result = subprocess.run(
        ["websocat", "-n1", "wss://testnet-rpc.lichen.network/ws"],
        input=payload,
        text=True,
        capture_output=True,
        timeout=6,
    )
    if result.returncode != 0 or '"result"' not in result.stdout:
        raise SystemExit(f"WebSocket attempt {i} failed: {result.stderr or result.stdout}")
print("WebSocket subscribeSlots: 10/10")
PY

python3 - <<'PY'
import json
import time
import urllib.request

base = "https://testnet-rpc.lichen.network"
deadline = time.time() + 90
last_error = None

def get_json(path):
    request = urllib.request.Request(
        base + path,
        headers={
            "Accept": "application/json",
            "User-Agent": "lichen-deploy-smoke/1.0",
        },
    )
    with urllib.request.urlopen(request, timeout=8) as response:
        return json.loads(response.read().decode())

while time.time() < deadline:
    try:
        oracle = get_json("/api/v1/oracle/prices")
        feeds = {feed.get("asset"): feed for feed in oracle.get("data", {}).get("feeds", [])}
        bad = []
        for asset in ("wSOL", "wETH", "wBNB"):
            feed = feeds.get(asset) or {}
            if int(feed.get("slot") or 0) <= 0 or feed.get("stale") is True:
                bad.append(f"{asset}:slot={feed.get('slot')} stale={feed.get('stale')}")
        candles = get_json("/api/v1/pairs/2/candles?interval=60&limit=4")
        candle_rows = candles.get("data") or []
        if not bad and candle_rows:
            print(
                "DEX oracle/candle smoke: "
                f"wSOL slot={feeds['wSOL'].get('slot')} "
                f"wSOL price={feeds['wSOL'].get('price')} "
                f"latest 1m close={candle_rows[-1].get('close')}"
            )
            break
        last_error = "; ".join(bad) or "missing wSOL 1m candles"
    except Exception as exc:
        last_error = str(exc)
    time.sleep(3)
else:
    raise SystemExit(f"DEX oracle/candle smoke failed: {last_error}")
PY
```

Also confirm any advertised dedicated WS aliases are on the same trusted edge path. With `tls internal`, DNS must resolve to the edge, not directly to validator VPS IPs:

```bash
dig +short testnet-rpc.lichen.network
dig +short testnet-ws.lichen.network
websocat -n1 wss://testnet-ws.lichen.network
```

If `testnet-ws.lichen.network` or `ws.lichen.network` resolves directly to validator IPs and presents an untrusted origin certificate, do not advertise it to clients. Use the RPC-hosted `/ws` endpoint until DNS is proxied through the edge or the origin has a public CA certificate.

If production portals are expected to stay usable, run the same checks against `https://rpc.lichen.network` and `wss://rpc.lichen.network/ws` too. Production portals default to `mainnet`, so a dead mainnet origin will surface as frontend CORS, incident-status, signed-metadata, and missing-contract-address errors even while `testnet-rpc.lichen.network` is healthy.

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

---

## Oracle price feed configuration

The oracle price feeder is **built into the validator binary** — there is no separate oracle service or market-maker process. Every running `lichen-validator` instance spawns `spawn_oracle_price_feeder()` which:

1. Opens a WebSocket to Binance for real-time aggregate trades (`solusdt`, `ethusdt`, `bnbusdt`).
2. Falls back to REST polling (`/api/v3/ticker/price`) every 5 seconds if the WS connection drops.
3. Stores prices in shared atomics (`SharedOraclePrices`).
4. Submits oracle-attestation transactions (system opcode 30) into the mempool every 5 seconds.
5. Broadcasts WS ticker and candle events to connected DEX frontend clients.

### Env vars

| Variable | Default | Description |
|----------|---------|-------------|
| `LICHEN_ORACLE_WS_URL` | `wss://stream.binance.com:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade` | Binance WebSocket stream |
| `LICHEN_ORACLE_REST_URL` | `https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]` | Binance REST fallback |
| `LICHEN_DISABLE_ORACLE` | unset | Set to `1` to disable the oracle entirely |

### US VPS geo-block

`api.binance.com` and `stream.binance.com` return HTTP 451 (Unavailable For Legal Reasons) from US IP addresses. If the validator is hosted on a US VPS, you **must** override both URLs to use Binance US:

```
LICHEN_ORACLE_WS_URL=wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade
LICHEN_ORACLE_REST_URL=https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]
```

`deploy/setup.sh` auto-detects OVH US hosts (IP prefix `15.204.*`) and writes these overrides automatically. For other US hosting providers, manually uncomment or add the env vars in `/etc/lichen/env-<net>`.

### Diagnosing silent oracle failures

If the DEX shows static prices that never move:

1. Check validator logs for Binance connection errors:
   ```bash
   sudo journalctl -u lichen-validator-testnet --no-pager | grep -i 'oracle\|binance\|price' | tail -20
   ```
2. Test the REST endpoint from the VPS:
   ```bash
   curl -sf 'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT"]' || echo "BLOCKED"
   curl -sf 'https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT"]' || echo "BLOCKED"
   ```
3. Verify env vars are loaded:
   ```bash
   grep ORACLE /etc/lichen/env-testnet
   ```

### Genesis price seeding

The genesis builder reads `GENESIS_SOL_USD`, `GENESIS_ETH_USD`, and `GENESIS_BNB_USD` env vars to seed initial oracle prices into the genesis state. If `api.binance.com` is blocked on the genesis host, fetch the prices from a trusted fallback before running `lichen-genesis`:

```bash
# From a non-US machine or your local dev box:
curl -s 'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]'
# Then export on the genesis host:
export GENESIS_SOL_USD=170.50
export GENESIS_ETH_USD=2650.00
export GENESIS_BNB_USD=620.00
```

Bridge and oracle committees are also genesis state. A clean 3-validator deployment must pre-generate all three validator keypairs before `lichen-genesis`, then pass each planned validator pubkey with both `--bridge-validator <pubkey>` and `--oracle-operator <pubkey>`. Do not patch these committees after genesis for a clean reset.

---

## Release signing key management (critical)

### The canonical signing key

The repo ships `keypairs/release-signing-key.json` — this is the **only** signing keypair whose public key (`8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`) is hardcoded in every frontend portal's `shared/utils.js` as `LICHEN_SIGNED_METADATA_SIGNERS`.

### The fatal mistake: generating keys on VPS

**NEVER run `scripts/generate-release-keys.sh` on a VPS.** That script creates a brand-new keypair with a different public key. If you use the VPS-generated key to sign metadata, every frontend portal will reject the manifest because the signer doesn't match the hardcoded expected signer.

### Correct signing key deployment

Copy the repo key to each VPS:

```bash
# From your local repo checkout:
scp -P 2222 keypairs/release-signing-key.json ubuntu@<VPS_IP>:~/release-signing-key.json

# On the VPS:
sudo install -m 640 -o root -g lichen \
  ~/release-signing-key.json \
  /etc/lichen/secrets/release-signing-keypair-testnet.json
rm ~/release-signing-key.json
```

If the US VPS has SFTP disabled:

```bash
cat keypairs/release-signing-key.json | ssh -p 2222 ubuntu@15.204.229.189 'cat > ~/release-signing-key.json'
ssh -p 2222 ubuntu@15.204.229.189 'sudo install -m 640 -o root -g lichen ~/release-signing-key.json /etc/lichen/secrets/release-signing-keypair-testnet.json && rm ~/release-signing-key.json'
```

### Verification

After deploying, verify the signed metadata manifest uses the expected signer:

```bash
curl -s http://127.0.0.1:8899 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' \
  | python3 -c "
import sys, json
d = json.load(sys.stdin)
r = d['result']
print(f\"Signer: {r['signer']}\")
assert r['signer'] == '8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk', 'WRONG SIGNER KEY'
p = r['payload']
print(f\"Symbols: {len(p.get('symbol_registry', []))}\")"
```

---

## Secrets distribution for joining validators

When EU and SEA also run custody/faucet service roles, these service files must be copied from the genesis VPS. These are not validator join requirements, and they are not sent to external validators:

| File | Source | Purpose |
|------|--------|---------|
| `custody-master-seed-testnet.txt` | Genesis VPS `/etc/lichen/secrets/` | Custody HD wallet root |
| `custody-deposit-seed-testnet.txt` | Genesis VPS `/etc/lichen/secrets/` | Custody deposit address derivation |
| `release-signing-keypair-testnet.json` | Repo `keypairs/release-signing-key.json` | Signed metadata manifest signing |
| `signed-metadata-manifest-testnet.json` | Genesis VPS `/etc/lichen/` | Pre-generated signed manifest |
| `custody-treasury-testnet.json` | Genesis VPS `/etc/lichen/` | Custody wrapped-token operational key |
| `faucet-keypair-testnet.json` | Genesis VPS `/var/lib/lichen/` | Testnet faucet funding key |

Copy procedure (genesis VPS → joining VPS):

```bash
# From genesis VPS:
scp -P 2222 /etc/lichen/secrets/custody-master-seed-testnet.txt ubuntu@<JOINING_IP>:~/
scp -P 2222 /etc/lichen/secrets/custody-deposit-seed-testnet.txt ubuntu@<JOINING_IP>:~/
scp -P 2222 /etc/lichen/signed-metadata-manifest-testnet.json ubuntu@<JOINING_IP>:~/

# On joining VPS:
sudo install -m 640 -o root -g lichen ~/custody-master-seed-testnet.txt /etc/lichen/secrets/
sudo install -m 640 -o root -g lichen ~/custody-deposit-seed-testnet.txt /etc/lichen/secrets/
sudo install -m 640 -o root -g lichen ~/signed-metadata-manifest-testnet.json /etc/lichen/
rm ~/custody-master-seed-testnet.txt ~/custody-deposit-seed-testnet.txt ~/signed-metadata-manifest-testnet.json
```

Do not distribute `genesis-wallet.json` or `genesis-keys/` to validator joiners. Raw `requestAirdrop` on an independent validator may report `"Treasury keypair not configured"`; the faucet service is the supported funding path.

---

## Complete clean-slate VPS redeployment checklist

This is the full step-by-step procedure for stopping everything, flushing all state, and redeploying from scratch so VPSes match the local 3-validator setup exactly.

Last exercised successfully on 2026-04-27 with signed release `v0.5.20`: the GitHub Release archive was installed on all three VPSes, `seed-02` and `seed-03` started from empty chain state, verified canonical block 0, synced/replayed from peers, and the final verifier reported all three validators healthy with bridge `3/2`, oracle `4` feeds plus `12` native attestations, empty faucet history, and 28 manifest symbols.

### One-command automated redeploy (recommended)

```bash
export LICHEN_OWNER_APPROVED_RESET='owner-approved:testnet:15.204.229.189,37.59.97.61,15.235.142.253'
export LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM='clean-slate:testnet:15.204.229.189,37.59.97.61,15.235.142.253'
export LICHEN_RELEASE_TAG=v0.5.36
bash scripts/clean-slate-redeploy.sh testnet
```

For mainnet, use the same host list with `owner-approved:mainnet:...` and `clean-slate:mainnet:...`, then run `bash scripts/clean-slate-redeploy.sh mainnet`. This path is destructive and requires explicit owner approval before use.

When `LICHEN_RELEASE_TAG` is set, the script downloads the GitHub Release archives, verifies `SHA256SUMS`, installs `lichen-validator`, `lichen-genesis`, `lichen`, `zk-prove`, and bundled `seeds.json` from those archives, and verifies the installed validator hash. It must not overwrite those runtime binaries from any remote `target/release` directory. Custody/faucet service binaries are still built from the synced release source unless they are added to the release archive contract.

This is the canonical way to perform an owner-approved full reset. It is not the normal path for code-only releases; use `scripts/rolling-release-deploy.sh` for those. It performs ALL reset phases automatically in ~3 minutes:

Use this path for any partial bootstrap or slot-0 bootstrap mismatch observed on one or more validators (for example: local DB claims progress but fails the canonical slot-0 state import). The cleanup model is a full state flush, not partial state file surgery.

| Phase | What it does | Typical time |
|-------|-------------|-------------|
| 1. Stop | Stops all services on all 3 VPSes, opens UFW port for cross-VPS RPC | ~9s |
| 2. Flush | Removes all state, custody DB, manifests | ~7s |
| 3. Sync + Build | Rsyncs code to all VPSes, builds binaries + WASM contracts on genesis VPS, distributes WASM to joiners | ~37s |
| 4. Validator identities | Pre-generates the validator keypair on each VPS and uses those exact three pubkeys for bridge/oracle genesis committees | ~5s |
| 5. Genesis | Prepares wallet, fetches live prices, creates genesis block with seed-only consensus plus 3-key bridge/oracle committees, starts genesis validator | ~14s |
| 6. Post-genesis | Runs `vps-post-genesis.sh`, installs signing key, verifies bridge/oracle readiness via `first-boot-deploy.sh`, provisions custody seeds | ~13s |
| 7. Service secrets | Bundles only custody/faucet/signing service secrets and signed metadata, then distributes those to joining VPSes; no RocksDB chain state is copied | ~20s |
| 8. Start joiners | Starts validators from their pre-generated keypairs and empty chain state, waits for genesis/block sync from peers, starts network-specific custody, and starts faucet on testnet only | ~30s to 5m |
| 9. Verify | Checks health/slot/genesis treasury or joiner treasury absence/protocol bootstrap/faucet/manifest on all 3 nodes, plus Cloudflare RPC health/protocol and public faucet health | ~37s |

Key design decisions:
- **No chain-state distribution**: Joining validators do not receive `CURRENT`, `MANIFEST-*`, `*.sst`, `genesis-wallet.json`, `genesis-keys/`, `known-peers.json`, or consensus WAL from the seed. They fetch the authoritative genesis config from seed RPC using `seeds.json`, then replay/sync blocks through the normal network path. This is the same model external agent-operated validators use.
- **Bridge/oracle bootstrap**: All three validator pubkeys are generated before genesis and embedded as bridge validators plus oracle operators. Only the seed validator is in the slot-zero consensus set so block production can start before joiners are online.
- **WASM distribution**: WASM contracts are built only on the genesis VPS and distributed via tarball — not compiled independently on each VPS.
- **Atomic service secrets**: Service secrets (custody treasury, custody seeds, faucet keypair on testnet, signed metadata manifest, signing key) are bundled into a single tarball per joining VPS — no partial copies. Validator chain state and treasury genesis keys are not included.
- **Cloudflare verification is service-aware**: `testnet-rpc.lichen.network` may round-robin to independent joiner validators that correctly do not hold raw genesis treasury material. The clean-slate verifier must validate public funding through `https://faucet.lichen.network`, not raw `requestAirdrop` on every public RPC origin.
- **SSH retry**: All SSH operations retry 3 times with exponential backoff (3s, 6s, 12s).
- **Code delivery**: Uses `rsync` (not `git pull`) since VPSes don't have `.git`.

### Recovery decision tree (on-call)

Use this to map symptoms to action quickly:

1. On startup, if logs show `⚠️  Detected partial genesis replay state without a stored slot-0 block`, the node has triggered the built-in partial-bootstrap recovery. Wait for a single restart cycle; if it remains online and catches peers, continue monitoring.
2. If the same node keeps at slot 0 and repeats slot-0 bootstrap errors, check startup output for `No genesis config found`, `GENESIS_SYNC_INCOMPLETE_MARKER`, or non-zero tip + missing genesis. If confirmed, execute the owner-approved full reset path (stop all services, flush every validator state path, rerun clean-slate redeploy).
3. If only one validator was previously wiped, confirm the root cause is intentional restart/rebuild; if TOFU or key identity drift is suspected, preserve or restore that validator's `validator-keypair.json`/`home/.lichen/node_identity.json` before continuing, or run full redeploy for all validators.
4. If RPC/custody/faucet services fail after chain is healthy, do not touch state. Pause service start-up and fix service secret/config ownership/permissions first, then restart services.
5. If chain state appears inconsistent across all nodes (health diverges, repeated root errors, repeated stalls), escalate to manual incident review and perform an owner-approved VPS nuclear reset only after evidence is captured.

Prerequisites:
- SSH access to all 3 VPSes (port 2222, user `ubuntu`, key-based auth)
- `deploy/setup.sh` already run on all VPSes (systemd units, users, dirs exist)
- `keypairs/release-signing-key.json` present in repo
- Code committed and pushed to main

### Manual phase-by-phase procedure (for debugging)

If the automated script fails or you need to debug, follow these phases manually:

### Prerequisites

- Latest code committed and pushed
- All CI checks green
- SSH access to all 3 VPSes (port 2222, user `ubuntu`)
- `keypairs/release-signing-key.json` present in repo
- `LICHEN_KEYPAIR_PASSWORD` known (or will be auto-generated by setup.sh)

### Phase 1: Stop everything (all 3 VPSes)

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Stopping $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    sudo systemctl stop lichen-faucet 2>/dev/null || true
    sudo systemctl stop lichen-custody 2>/dev/null || true
    sudo systemctl stop lichen-validator-testnet 2>/dev/null || true
    echo "Services stopped"
  '
done
```

### Phase 2: Flush state (all 3 VPSes)

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Flushing $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    sudo rm -rf /var/lib/lichen/state-testnet
    sudo rm -rf /var/lib/lichen/.lichen
    sudo rm -rf /var/lib/lichen/custody-db
    sudo rm -f /etc/lichen/signed-metadata-manifest-testnet.json
    sudo rm -f /var/lib/lichen/faucet-keypair-testnet.json
    sudo rm -f /var/lib/lichen/airdrops.json
    echo "State flushed"
  '
done
```

### Phase 3: Rsync code to all VPSes

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Syncing to $VPS ==="
  rsync -az --delete \
    --exclude '.git' \
    --exclude 'target' \
    --exclude 'compiler/target' \
    --exclude 'data' \
    --exclude 'logs' \
    --exclude 'node_modules' \
    --exclude 'dist' \
    -e 'ssh -p 2222' \
    ./ ubuntu@$VPS:~/lichen/
done
```

### Phase 4: Build on all VPSes

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Building on $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    cd ~/lichen
    source ~/.cargo/env
    # Touch source files so cargo sees them as newer than stale remote artifacts
    find . \( -path ./target -o -path ./compiler/target -o -path ./node_modules \) -prune -o \
      -type f \( -name "*.rs" -o -name "Cargo.toml" -o -name "Cargo.lock" \) -exec touch {} +
    cargo build --release --bin lichen-validator --bin lichen-genesis --bin lichen-faucet --bin lichen-custody --bin lichen --bin zk-prove
    ./scripts/build-all-contracts.sh
    echo "Build complete"
  '
done
```

### Phase 5: Run setup.sh on all VPSes

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Setup on $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    cd ~/lichen
    sudo bash deploy/setup.sh testnet
  '
done
```

This auto-detects the US VPS and configures Binance US oracle endpoints.

### Phase 6: Copy signing key to all VPSes

```bash
for VPS in 37.59.97.61 15.235.142.253; do
  scp -P 2222 keypairs/release-signing-key.json ubuntu@$VPS:~/release-signing-key.json
  ssh -p 2222 ubuntu@$VPS '
    sudo install -m 640 -o root -g lichen ~/release-signing-key.json /etc/lichen/secrets/release-signing-keypair-testnet.json
    rm ~/release-signing-key.json
  '
done

# US VPS (SFTP may be disabled):
cat keypairs/release-signing-key.json | ssh -p 2222 ubuntu@15.204.229.189 'cat > ~/release-signing-key.json'
ssh -p 2222 ubuntu@15.204.229.189 '
  sudo install -m 640 -o root -g lichen ~/release-signing-key.json /etc/lichen/secrets/release-signing-keypair-testnet.json
  rm ~/release-signing-key.json
'
```

### Phase 7: Genesis on US VPS (primary validator)

```bash
ssh -p 2222 ubuntu@15.204.229.189 '
  cd ~/lichen
  source ~/.cargo/env

  # Start once to generate validator keypair
  sudo systemctl start lichen-validator-testnet
  sleep 3
  VALIDATOR_PUBKEY=$(sudo python3 -c "import json; print(json.load(open(\"/var/lib/lichen/state-testnet/validator-keypair.json\"))[\"publicKeyBase58\"])")
  echo "Validator pubkey: $VALIDATOR_PUBKEY"
  sudo systemctl stop lichen-validator-testnet

  # Preserve validator keypair, wipe state
  sudo install -D -m 600 -o lichen -g lichen \
    /var/lib/lichen/state-testnet/validator-keypair.json \
    /var/lib/lichen/validator-keypair-testnet.json
  sudo rm -rf /var/lib/lichen/state-testnet
  sudo rm -rf /var/lib/lichen/.lichen
  sudo install -d -m 750 -o lichen -g lichen /var/lib/lichen/state-testnet
  sudo install -m 600 -o lichen -g lichen \
    /var/lib/lichen/validator-keypair-testnet.json \
    /var/lib/lichen/state-testnet/validator-keypair.json

  # Prepare wallet
  LICHEN_KEYPAIR_PASSWORD=$(grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-testnet | cut -d= -f2-)
  export LICHEN_KEYPAIR_PASSWORD
  cd ~/lichen
  sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
    LICHEN_KEYPAIR_PASSWORD="$LICHEN_KEYPAIR_PASSWORD" \
    LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
    ./scripts/generate-genesis.sh \
    --network testnet --prepare-wallet --output-dir /var/lib/lichen/genesis-keys-testnet

  # Create genesis DB. For mainnet, pass --genesis-prices-file or ensure
  # Binance/CoinGecko live price access is available; compiled market defaults
  # are testnet/dev fallback only.
  sudo -u lichen HOME=/var/lib/lichen LICHEN_HOME=/var/lib/lichen LICHEN_CONTRACTS_DIR=/var/lib/lichen/contracts \
    LICHEN_KEYPAIR_PASSWORD="$LICHEN_KEYPAIR_PASSWORD" \
    LICHEN_GENESIS_BIN=/usr/local/bin/lichen-genesis \
    ./scripts/generate-genesis.sh \
    --network testnet \
    --db-path /var/lib/lichen/state-testnet \
    --wallet-file /var/lib/lichen/genesis-keys-testnet/genesis-wallet.json \
    --initial-validator "$VALIDATOR_PUBKEY" \
    --bridge-validator "$US_VALIDATOR_PUBKEY" --oracle-operator "$US_VALIDATOR_PUBKEY" \
    --bridge-validator "$EU_VALIDATOR_PUBKEY" --oracle-operator "$EU_VALIDATOR_PUBKEY" \
    --bridge-validator "$SEA_VALIDATOR_PUBKEY" --oracle-operator "$SEA_VALIDATOR_PUBKEY"

  # Start the genesis validator
  sudo systemctl start lichen-validator-testnet
  sleep 5
  curl -s http://127.0.0.1:8899 -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}"
'
```

### Phase 8: Post-genesis on US VPS

```bash
ssh -p 2222 ubuntu@15.204.229.189 '
  cd ~/lichen

  # Run vps-post-genesis to copy keypairs to system paths
  sudo bash scripts/vps-post-genesis.sh testnet

  # Make the release signing key readable for first-boot-deploy
  sudo cp /etc/lichen/secrets/release-signing-keypair-testnet.json ~/release-signing-keypair-testnet.json
  sudo chown $(whoami):$(whoami) ~/release-signing-keypair-testnet.json
  chmod 600 ~/release-signing-keypair-testnet.json

  # Run first-boot-deploy
  SIGNED_METADATA_KEYPAIR=$HOME/release-signing-keypair-testnet.json \
    DEPLOY_NETWORK=testnet ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:8899 --skip-build

  rm -f ~/release-signing-keypair-testnet.json

  # Install the signed metadata manifest
  sudo install -m 640 -o root -g lichen \
    ~/lichen/signed-metadata-manifest-testnet.json \
    /etc/lichen/signed-metadata-manifest-testnet.json

  # Restart validator to pick up manifest
  sudo systemctl restart lichen-validator-testnet
  sleep 3

  # Verify
  curl -s http://127.0.0.1:8899 -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignedMetadataManifest\",\"params\":[]}" \
    | python3 -c "import sys,json; d=json.load(sys.stdin); r=d[\"result\"]; p=r[\"payload\"]; print(f\"Signer: {r['signer']}, Symbols: {len(p.get('symbol_registry',[]))}\")"

  # Provision custody seeds
  sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-master-seed-testnet.txt"
  sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-deposit-seed-testnet.txt"
  sudo chown root:lichen /etc/lichen/secrets/custody-*-seed-testnet.txt
  sudo chmod 640 /etc/lichen/secrets/custody-*-seed-testnet.txt

  # Start custody and faucet
  sudo systemctl start lichen-custody
  sudo systemctl start lichen-faucet
'
```

### Phase 9: Distribute secrets to joining VPSes

Joining validators sync the blockchain from the genesis seed node via P2P — **do NOT copy state**.
Each joining validator starts with an empty `state-testnet/` directory and catches up by
requesting blocks from the seed node(s) listed in `seeds.json`. This is the production-correct
flow because future agent-operated validators will join the same way.

Only secrets and the signed metadata manifest need to be distributed:

```bash
for VPS in 37.59.97.61 15.235.142.253; do
  echo "=== Distributing secrets to $VPS ==="

  # Ensure the state directory exists but is empty (validator creates its own keypair on first start)
  ssh -p 2222 ubuntu@$VPS '
    sudo rm -rf /var/lib/lichen/state-testnet
    sudo mkdir -p /var/lib/lichen/state-testnet
    sudo chown -R lichen:lichen /var/lib/lichen/state-testnet
  '

  # Copy custody secrets
  ssh -p 2222 ubuntu@15.204.229.189 "sudo cat /etc/lichen/secrets/custody-master-seed-testnet.txt" \
    | ssh -p 2222 ubuntu@$VPS "sudo bash -c 'cat > /etc/lichen/secrets/custody-master-seed-testnet.txt && chown root:lichen /etc/lichen/secrets/custody-master-seed-testnet.txt && chmod 640 /etc/lichen/secrets/custody-master-seed-testnet.txt'"

  ssh -p 2222 ubuntu@15.204.229.189 "sudo cat /etc/lichen/secrets/custody-deposit-seed-testnet.txt" \
    | ssh -p 2222 ubuntu@$VPS "sudo bash -c 'cat > /etc/lichen/secrets/custody-deposit-seed-testnet.txt && chown root:lichen /etc/lichen/secrets/custody-deposit-seed-testnet.txt && chmod 640 /etc/lichen/secrets/custody-deposit-seed-testnet.txt'"

  # Copy signed metadata manifest
  ssh -p 2222 ubuntu@15.204.229.189 "sudo cat /etc/lichen/signed-metadata-manifest-testnet.json" \
    | ssh -p 2222 ubuntu@$VPS "sudo bash -c 'cat > /etc/lichen/signed-metadata-manifest-testnet.json && chown root:lichen /etc/lichen/signed-metadata-manifest-testnet.json && chmod 640 /etc/lichen/signed-metadata-manifest-testnet.json'"

  # Copy custody treasury keypair
  ssh -p 2222 ubuntu@15.204.229.189 "sudo cat /etc/lichen/custody-treasury-testnet.json" \
    | ssh -p 2222 ubuntu@$VPS "sudo bash -c 'cat > /etc/lichen/custody-treasury-testnet.json && chown lichen:lichen /etc/lichen/custody-treasury-testnet.json && chmod 600 /etc/lichen/custody-treasury-testnet.json'"

  # Copy faucet keypair
  ssh -p 2222 ubuntu@15.204.229.189 "sudo cat /var/lib/lichen/faucet-keypair-testnet.json" \
    | ssh -p 2222 ubuntu@$VPS "sudo bash -c 'cat > /var/lib/lichen/faucet-keypair-testnet.json && chown lichen:lichen /var/lib/lichen/faucet-keypair-testnet.json && chmod 600 /var/lib/lichen/faucet-keypair-testnet.json'"

  echo "Done: $VPS"
done
```

### Phase 10: Start joining VPSes

```bash
for VPS in 37.59.97.61 15.235.142.253; do
  echo "=== Starting $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    sudo systemctl start lichen-validator-testnet
    sleep 5
    curl -s http://127.0.0.1:8899 -X POST -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}"
    echo ""
    sudo systemctl start lichen-custody
    sudo systemctl start lichen-faucet
    echo "Services started"
  '
done
```

### Phase 11: Verify everything

```bash
for VPS in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== Verifying $VPS ==="
  ssh -p 2222 ubuntu@$VPS '
    echo "--- Health ---"
    curl -s http://127.0.0.1:8899 -X POST -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}"
    echo ""
    echo "--- Signed Metadata ---"
    curl -s http://127.0.0.1:8899 -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignedMetadataManifest\",\"params\":[]}" \
      | python3 -c "import sys,json; d=json.load(sys.stdin); r=d[\"result\"]; p=r[\"payload\"]; print(f\"Signer: {r['signer']}, Symbols: {len(p.get('symbol_registry',[]))}\")" 2>/dev/null || echo "MANIFEST MISSING"
    echo ""
    echo "--- Pairs ---"
    curl -s http://127.0.0.1:8899/api/v1/pairs | python3 -c "import sys,json; d=json.load(sys.stdin); print(f\"{len(d)} pairs\")" 2>/dev/null || echo "PAIRS MISSING"
    echo ""
    echo "--- Oracle Prices ---"
    curl -s http://127.0.0.1:8899 -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getOraclePrices\",\"params\":[]}" 2>/dev/null | head -c 200
    echo ""
  '
  echo ""
done
```

Also verify external endpoints:

```bash
for HOST in testnet-rpc.lichen.network; do
  echo "=== External: $HOST ==="
  curl -s "https://$HOST/" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
  echo ""
  curl -s "https://$HOST/" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' \
    | python3 -c "import sys,json; d=json.load(sys.stdin); r=d['result']; print(f\"Signer: {r['signer']}\")" 2>/dev/null || echo "MANIFEST MISSING"
  echo ""
done
```

---

## Deployment postmortem: known pitfalls

This section documents actual deployment failures and their root causes, so they are never repeated.

### Pitfall 1: Release signing key mismatch

**Symptom**: DEX shows "Missing contract addresses", all frontends fail to load token metadata.

**Root cause**: `scripts/generate-release-keys.sh` was run on the VPS, creating a new keypair. The VPS signed the metadata manifest with a different key than what frontends expect.

**Fix**: Always copy `keypairs/release-signing-key.json` from the repo to `/etc/lichen/secrets/release-signing-keypair-testnet.json`. Never generate new keys on VPS.

**Prevention**: `deploy/setup.sh` does not generate signing keys. The operator must provision the canonical key manually.

### Pitfall 2: US VPS oracle geo-block

**Symptom**: DEX prices are static — they load from genesis but never update. Local 3-validator cluster works fine.

**Root cause**: The US VPS at `15.204.229.189` cannot reach `api.binance.com` / `stream.binance.com` (HTTP 451 geo-block). The validator's oracle feeder silently fails, no attestation transactions are submitted, and prices never move.

**Fix**: Set `LICHEN_ORACLE_WS_URL` and `LICHEN_ORACLE_REST_URL` to binance.us endpoints in `/etc/lichen/env-testnet`.

**Prevention**: `deploy/setup.sh` now auto-detects US hosts and writes binance.us overrides.

### Pitfall 3: Stale deploy-manifest.json

**Symptom**: `first-boot-deploy.sh` fails or generates incorrect signed metadata.

**Root cause**: An old `deploy-manifest.json` from a previous deployment was carried into the new rsync. The script detects a mismatch with the live symbol registry.

**Fix**: Delete `deploy-manifest.json` and `signed-metadata-manifest-*.json` from the repo checkout on the VPS before running `first-boot-deploy.sh`.

### Pitfall 4: Cargo not in PATH on VPS

**Symptom**: `cargo build` fails with "command not found" over SSH.

**Root cause**: SSH non-login shells don't source `~/.cargo/env`. Running `ssh host 'cargo build'` fails.

**Fix**: Always prefix with `source ~/.cargo/env` in SSH commands.

### Pitfall 5: SFTP disabled on US VPS

**Symptom**: `scp` fails to the US VPS.

**Root cause**: OVH US VPS has SFTP subsystem disabled in sshd_config.

**Fix**: Use `cat file | ssh host 'cat > remote_file'` for file transfers to US VPS.

### Pitfall 6: Custody permission denied

**Symptom**: `lichen-custody` fails to start with "Permission denied" on seed files.

**Root cause**: Seed files provisioned as `root:root 600` instead of `root:lichen 640`.

**Fix**: Ensure `/etc/lichen/secrets` is `root:lichen 750` and all files inside are `root:lichen 640`.

### Pitfall 7: Missing LICHEN_KEYPAIR_PASSWORD in custody env

**Symptom**: Custody service can't read the encrypted treasury keypair.

**Root cause**: `deploy/setup.sh` writes the password to both `env-testnet` and `custody-env`, but if setup.sh is re-run without preserving the original password, the custody env gets a new password that doesn't match the encrypted keypair.

**Fix**: Setup.sh preserves existing passwords. If you need to change the password, you must re-encrypt all keypair files.

### Pitfall 8: Contract WASM binary mismatch across validators

**Symptom**: Joining validators fail to sync — state root mismatch at genesis block.

**Root cause**: Contracts were rebuilt only on the genesis VPS, producing different WASM hashes than the joining VPSes.

**Fix**: Either (a) rsync pre-built WASM artifacts to all VPSes and never rebuild, or (b) run `./scripts/build-all-contracts.sh` on ALL VPSes before genesis. Verify the bundle hash matches:

```bash
command find /var/lib/lichen/contracts -maxdepth 2 -name '*.wasm' | sort | xargs shasum -a 256 | shasum -a 256
```

### Pitfall 9: Full root filesystem makes RPC stale

**Symptom**: One VPS is online at the process level but serves an old slot, public RPC intermittently returns stale chain data, and validator logs contain RocksDB write failures with `No space left on device`.

**Root cause**: Old non-live chain-state backup directories and unbounded sudo I/O logs filled `/`. Once RocksDB could not flush writes, the node stopped advancing even though the RPC process still answered some requests.

**Fix**: Remove non-live state backups from the root filesystem, vacuum bounded logs, restart the affected validator, and verify `getHealth` reports `status:"ok"` with `disk.critical:false`.

**Prevention**: Re-run `deploy/setup.sh` on every VPS before a reset or launch, then complete the "VPS disk and log guardrails" preflight. Do not route public traffic to a node whose readiness endpoint reports `stale_tip` or critical disk.

### Pitfall 10: Rolling release leaves validators on different committed tips

**Symptom**: Validators report different latest slots, BFT repeatedly logs `Syncing to exact network tip`, and a node rejects a peer block with `state-root mismatch`.

**Root cause**: A validator signed consensus votes for a block before locally replaying the full proposal against its canonical pre-state. That is not a valid production BFT flow: validators must process the proposal and verify parent hash, validator-set hash, transaction root, fee metadata, and state root before prevoting.

**Fix**: Stop every validator immediately and keep `/var/lib/lichen/state-<net>` plus journals for evidence. Ship a new signed release with the proposal-validation gate fixed. For testnet, recover with the owner-approved clean-slate redeploy so the seed creates genesis and joiners sync from peers. For mainnet, do not reset without a separate governance/incident decision; first evaluate whether a finalized-height rollback procedure exists.

**Prevention**: Treat proposal execution before prevote as a release-blocking invariant. A rolling deploy health gate must fail on stale block age, split tips, or state-root mismatch, and operators must not heal by copying another validator's RocksDB directory.

### Pitfall 11: Rolling release installs a new binary but keeps an old process alive

**Symptom**: `/usr/local/bin/lichen-validator` has the new release hash, but one validator still serves behavior from the previous release. `systemctl status` shows an old `ExecMainStartTimestamp`, and `/proc/<pid>/exe` points at `/usr/local/bin/lichen-validator (deleted)` with the old executable hash.

**Root cause**: The release archive was installed on disk, but the validator service did not replace the running process. A health-only rolling gate can pass because the old process still answers RPC.

**Fix**: Stop the affected service, kill the service control group only if it remains active after stop, start it again, and verify `/proc/<pid>/exe` for the main and supervised validator processes hashes to the installed release binary. Do not reset chain state or copy RocksDB state for this service-process issue.

**Prevention**: Rolling deploys must fail unless the service PID/start timestamp changes and all running validator processes in the service execute the expected signed-release hash.

### Pitfall 12: Commit-certificate subsets must not affect state

**Symptom**: A validator accepts a fee-bearing block, then rejects the next block with `state-root mismatch`. The rejected next block may be an empty liveness block because it only exposes the state divergence from the previous committed block.

**Root cause**: Post-block fee distribution used the locally observed `commit_signatures` subset as an input to account balances. That subset is finality evidence, not canonical block state: validators are allowed to commit as soon as they observe any valid two-thirds supermajority, so different validators can persist different signature subsets for the same block.

**Fix**: Ship a validator release that derives all fee-recipient state from canonical block data plus the active stake pool, with deterministic ordering. Do not copy RocksDB state between validators to hide the divergence.

**Prevention**: No state-root-affecting path may depend on local vote aggregators, locally collected prevotes/precommits, commit-certificate subset size/order, wall-clock arrival order, RPC routing, or other observer-local evidence. Release tests must include "same block, different commit-signature subsets, same state root" coverage for any post-block accounting path.

### Pitfall 13: Stale Caddy ingress breaks WebSocket intermittently

**Symptom**: HTTPS JSON-RPC is healthy, but `wss://testnet-rpc.lichen.network/ws` intermittently hangs or fails to subscribe.

**Root cause**: The origin Caddyfile drifted from the checked-in repo-managed ingress and still proxied `/ws` to another validator's raw `:8900` listener. Raw RPC/WS ports are intentionally not public ingress, so cross-origin proxy attempts can hang even while each validator is locally healthy.

**Fix**: Reinstall the checked-in Caddy fragments with `deploy/setup.sh <net>` or rerun the clean-slate redeploy script version that installs repo-managed Caddy ingress after syncing code, then restart Caddy.

**Prevention**: Treat Caddy as deployment state, not hand-edited host state. After every reset or launch, compare `/etc/caddy/Caddyfile` against `deploy/Caddyfile.common` plus the network fragment and run the public `subscribeSlots` WebSocket smoke test 10/10.

### Pitfall 14: Dedicated WS hostname bypasses edge TLS

**Symptom**: `wss://testnet-rpc.lichen.network/ws` works, but `wss://testnet-ws.lichen.network` or `wss://ws.lichen.network` fails with an untrusted certificate.

**Root cause**: The dedicated WS hostname resolves directly to validator VPS IPs while the checked-in Caddy config uses `tls internal` for Cloudflare-origin traffic. Public clients then see the origin-only certificate instead of the trusted edge certificate.

**Fix**: Prefer the RPC-hosted WS endpoint (`wss://testnet-rpc.lichen.network/ws` or `wss://rpc.lichen.network/ws`) in clients. If the dedicated hostname is still advertised, proxy it through the trusted edge or switch that origin hostname to a public CA certificate, then rerun the WSS smoke test.

**Prevention**: DNS and TLS are part of the release gate. For every advertised WSS hostname, `dig +short` must show the intended edge path when `tls internal` is used, and `websocat -n1 <url>` must pass from outside the VPS network.

### Pitfall 15: Native oracle consensus remains at genesis while ticker moves

**Symptom**: DEX pair prices move through `ticker:<pair_id>` WebSocket updates, but `/api/v1/oracle/prices` shows wrapped assets at `slot:0` with `stale:true`, and `/api/v1/pairs/<id>/candles?interval=60` keeps returning flat genesis-price candles after a reset.

**Root cause**: Oracle attestation transactions are included in blocks, but committed replay failed to persist opcode-30 oracle attestation side effects. The live ticker path can still move because it broadcasts from the validator's local market feed, while DEX candles are written from committed native consensus oracle prices.

**Fix**: Ship a validator release that persists oracle attestation and consensus-price side effects exactly once after the canonical committed transaction batch, then restart through the runbook. Do not patch the DEX frontend, synthesize candles, or seed fake history.

**Prevention**: After every reset, launch, or rolling release, run the Step 8 DEX oracle/candle smoke. The gate must prove that wrapped-asset native consensus oracle slots advance past genesis and that 1m candles are present through the same public RPC path used by browsers.

### Pitfall 16: TOFU identity prevents rejoining after state wipe

**Symptom**: Wiped validator responds on RPC but stays at slot 0. P2P connections from other validators close immediately: `Failed to accept stream: closed by peer: 0`.

**Root cause**: The wiped validator generates a new keypair and P2P identity. Other validators have the old identity cached in their TOFU (Trust On First Use) store at `data/state-<port>/home/.lichen/peer_identities.json`. The TOFU check rejects the new identity as an impostor.

Also, the wiped validator's new pubkey registers as a separate entry in the validator set. With N+1 validators and only N-1 online (original minus the ghost), BFT quorum (2/3+) may be unreachable.

**Fix for local dev**: Remove the wiped validator's entry from all other validators' TOFU stores, then restart all validators from scratch:

```bash
# Remove stale TOFU entries (example: V2 on port 7002 was wiped)
python3 -c "
import json
for v in ['state-7001', 'state-7003']:
    path = f'data/{v}/home/.lichen/peer_identities.json'
    with open(path) as f:
        d = json.load(f)
    if '127.0.0.1:7002' in d:
        del d['127.0.0.1:7002']
        with open(path, 'w') as f:
            json.dump(d, f, indent=2)
```

**Prevention**: If you must wipe a single validator, preserve its `validator-keypair.json` and `home/.lichen/node_identity.json` first and restore them after the wipe. Or wipe all validators and create a fresh genesis.

---

## Nuclear reset procedure

Use this when the validators have diverged beyond recovery (e.g. state root mismatch, stuck consensus, validator identity conflicts).

### Local nuclear reset

```bash
# 1. Stop everything
./lichen-stop.sh all
# Kill any lingering supervisors
pkill -9 -f validator-supervisor
pkill -9 -f lichen-validator

# 2. Wipe all state
rm -rf data/state-7001 data/state-7002 data/state-7003
mkdir -p data/state-7001 data/state-7002 data/state-7003

# 3. Start fresh — V1 creates genesis, then V2 and V3 sync
LICHEN_LOCAL_DEV=1 ./run-validator.sh testnet 1 --dev-mode &
sleep 15  # wait for genesis creation + first blocks
LICHEN_LOCAL_DEV=1 ./run-validator.sh testnet 2 --dev-mode &
LICHEN_LOCAL_DEV=1 ./run-validator.sh testnet 3 --dev-mode &

# 4. Verify all 3 are healthy and at the same slot
sleep 10
for port in 8899 8901 8903; do
  echo "Port $port:"
  curl -s http://localhost:$port -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
  echo ""
done
```

### VPS nuclear reset (all validators)

```bash
# 1. Stop services on ALL VPSes
for HOST in seed-01 seed-02 seed-03; do
  ssh -p 2222 ubuntu@$HOST.lichen.network \
    'sudo systemctl stop lichen-validator-testnet lichen-custody lichen-faucet'
done

# 2. Wipe state on ALL VPSes (preserve keypairs!)
for HOST in seed-01 seed-02 seed-03; do
  ssh -p 2222 ubuntu@$HOST.lichen.network '
    sudo cp /var/lib/lichen/state-testnet/validator-keypair.json /tmp/vk-backup.json
    sudo rm -rf /var/lib/lichen/state-testnet
    sudo rm -rf /var/lib/lichen/.lichen
    sudo install -d -m 750 -o lichen -g lichen /var/lib/lichen/state-testnet
    sudo install -m 600 -o lichen -g lichen /tmp/vk-backup.json \
      /var/lib/lichen/state-testnet/validator-keypair.json
    sudo rm /tmp/vk-backup.json
  '
done

# 3. Recreate genesis on the primary (US) VPS — follow Step 4 from VPS Runbook above

# 4. Run post-genesis deploy on the genesis VPS — follow Step 5

# 5. Copy signed metadata manifest to joining VPSes — follow Step 6

# 6. Start validators on ALL VPSes
for HOST in seed-01 seed-02 seed-03; do
  ssh -p 2222 ubuntu@$HOST.lichen.network \
    'sudo systemctl start lichen-validator-testnet'
done

# 7. Start custody and faucet on genesis VPS
ssh -p 2222 ubuntu@seed-01.lichen.network \
  'sudo systemctl start lichen-custody && sudo systemctl start lichen-faucet'

# 8. Verify all 3 are healthy
for HOST in seed-01 seed-02 seed-03; do
  echo "=== $HOST ==="
  ssh -p 2222 ubuntu@$HOST.lichen.network \
    'curl -s http://127.0.0.1:8899 -X POST -H "Content-Type: application/json" \
       -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}"'
  echo ""
done
```

Critical: always preserve `validator-keypair.json` during a nuclear reset. If you lose it, the genesis validator pubkey changes and a completely new genesis is required.

---

## Cloudflare Pages deployment

All frontend portals are deployed as static sites to Cloudflare Pages via Wrangler.

### Projects

| Portal | Pages project | Directory | Custom domain |
|--------|--------------|-----------|---------------|
| DEX | `lichen-network-dex` | `dex/` | `dex.lichen.network` |
| Wallet | `lichen-network-wallet` | `wallet/` | `wallet.lichen.network` |
| Explorer | `lichen-network-explorer` | `explorer/` | `explorer.lichen.network` |
| Faucet | `lichen-network-faucet` | `faucet/` | `faucet.lichen.network` |
| Marketplace | `lichen-network-marketplace` | `marketplace/` | `marketplace.lichen.network` |
| Developers | `lichen-network-developers` | `developers/` | `developers.lichen.network` |
| Programs | `lichen-network-programs` | `programs/` | `programs.lichen.network` |
| Monitoring | `lichen-network-monitoring` | `monitoring/` | `monitoring.lichen.network` |
| Website | `lichen-network-website` | `website/` | `lichen.network` |

### Deploy all frontends

```bash
for portal in dex wallet explorer faucet marketplace developers programs monitoring website; do
  echo "=== Deploying $portal ==="
  wrangler pages deploy "$portal/" \
    --project-name "lichen-network-$portal" \
    --branch main \
    --commit-dirty=true
  echo ""
done
```

### Deploy a single frontend

```bash
wrangler pages deploy dex/ --project-name lichen-network-dex --branch main --commit-dirty=true
```

### Shared configuration

All portals share `shared-config.js` which defines RPC endpoints, WebSocket URLs, and cross-portal links. When updating this file:

1. Edit the canonical copy in `dex/shared-config.js`
2. Copy to all other portals:

```bash
for dir in wallet explorer faucet marketplace developers programs monitoring website; do
  cp dex/shared-config.js "$dir/shared-config.js"
done
```

3. Verify all copies are identical:

```bash
shasum dex/shared-config.js wallet/shared-config.js explorer/shared-config.js \
  faucet/shared-config.js marketplace/shared-config.js developers/shared-config.js \
  programs/shared-config.js monitoring/shared-config.js website/shared-config.js
```

4. Redeploy all portals via the deploy loop above.

### Custom domains

Custom domains are managed in the Cloudflare Dashboard, not via Wrangler:

1. Go to Pages > project > Custom domains
2. Add the domain (e.g. `dex.lichen.network`)
3. Cloudflare auto-creates a CNAME record if DNS is managed by Cloudflare

### Faucet architecture

The faucet has two separate components served on different domains:

- **`lichen-network-faucet.pages.dev`** (Cloudflare Pages): Static faucet portal (HTML/JS/CSS). This is the user-facing page.
- **`faucet.lichen.network`** (Cloudflare → Caddy → VPS port 9100): Faucet Rust/axum API service. This is the backend that dispenses LICN.

The static portal calls the API at `https://faucet.lichen.network/faucet/request`. This works because:

1. `shared-config.js` sets `faucet: 'https://faucet.lichen.network'` in production
2. `faucet.js` reads `LICHEN_CONFIG.faucet` as `FAUCET_API`
3. DNS for `faucet.lichen.network` routes through Cloudflare to the US VPS
4. Caddy (`Caddyfile.testnet-us`) reverse proxies all traffic to `127.0.0.1:9100`
5. The faucet-service CORS layer allows `https://faucet.lichen.network` and all portal origins

The faucet service only serves API endpoints (`/health`, `/faucet/config`, `/faucet/status`, `/faucet/airdrops`, `/faucet/request`). It does NOT serve static HTML — that comes from Cloudflare Pages.

Do NOT confuse the `faucet` key in `shared-config.js` with a portal URL — it is the API endpoint. The faucet portal is accessed via the Pages `.pages.dev` domain or a custom domain added to the Pages project.

---

## Incident Log

### 2026-04-09: Treasury keypair missing on seed-02/seed-03

**Symptom:** RPC `requestAirdrop` calls routed via Cloudflare round-robin intermittently failed with `"Treasury keypair not configured"`. DEX and marketplace tests on VPS showed 0 orders/trades because wallets only had ~20 LICN (genesis allocation) and could not fund contract operations.

**Root cause:** The `genesis-wallet.json` and `genesis-keys/` directory (containing the treasury keypair) are local artifacts created ONLY on the genesis VPS during genesis creation. They are NOT part of the blockchain state that syncs via P2P. The deployment pipeline had no step to distribute these files to joining VPSes. Seed-02 and seed-03 had empty `genesis-keys/` directories and no `genesis-wallet.json`, causing `load_treasury_keypair()` to return `None`.

**Superseded fix:** That incident was initially handled by copying genesis keys to all validators. That is no longer the production model. Joining validators must not need, receive, or rely on genesis wallet material. Faucet/custody service hosts may receive service-specific keys, but validator chain sync and consensus membership must work from the validator's own identity key plus `seeds.json`.

**Prevention:** Keep airdrop/faucet routing service-aware instead of treating every validator RPC as a treasury signer. External validators and independent joiner VPSes should report `"Treasury keypair not configured"` for raw `requestAirdrop` unless they intentionally run a faucet/treasury service.
