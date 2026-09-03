# Lichen 🦞⚡

**A post-quantum-native blockchain built for agents and programmable markets.**

Ultra-low fees · Sub-second BFT block commitment · Agent-native identity · Multi-language SDKs

[![License: Apache--2.0%20%2B%20MIT](https://img.shields.io/badge/License-Apache--2.0%20%2B%20MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.88+-00C9DB.svg)](https://www.rust-lang.org)

**Candidate release line:** `v0.5.274`; the installed signed testnet release is
`v0.5.272`, with `v0.5.265` retained as the restart-safe rollback anchor.
Official installable artifacts are the published
GitHub release archives whose checksums, detached ML-DSA signature, release
trust anchor, and provenance attestations all verify. The candidate adds the
coherent hot-to-Archive-V2 checkpoint path, certificate-normalized public
history, fail-closed role bootstrap, and signed range-bound retirement tooling
needed to move an existing network fully onto Archive V2. Treat those changes
as unreleased until the exact tag, signature, provenance, and release gates pass.

The signed `v0.5.272` release accepts the legacy deployed contract ABI field
`name` while continuing to serialize the canonical `contract` field. Its
coordinated preserved-chain repair targets the real validator `--db-path`, and
its bounded service stop verifies that the complete systemd control group is
empty before installing a signed binary. It additionally makes the strict DEX
journey establish and verify a fresh active-validator quorum price band before
each pair-1 CLOB phase. This keeps the production 750-slot stale-price rejection
intact while removing runner-speed timing from release qualification. The
immutable `v0.5.271` workflow failed this gate and produced no deployable
release. These changes preserve the existing Testnet chain and do not authorize
a reset or state copy.

The immutable `v0.5.273` candidate corrected two production-only Archive V2
checkpoint admission failures exposed by the preserved 12-million-block
Testnet. Explicit pre-activation hot repair is reachable at the ordinary
1,000-slot checkpoint boundary, while catalog-bound compaction remains at
10,000 slots. Checkpoint
headroom meters only newly materialized bounded rows, WAL, and compaction output;
it no longer charges twice for more than 113 GB of inherited SSTs that RocksDB
hard-links without allocating new data blocks. The build remains fail-closed,
budgeted and atomically staged. Its first tag workflow nevertheless failed
closed after a fresh verified-cache node imported and checkpointed a verified
snapshot but exited before role admission. It was not signed or deployed.

The `v0.5.274` candidate keeps that imported state non-live until its durable
rollback transaction has been removed, retries transient cleanup failures, and
emits the terminal reason synchronously if cleanup remains impossible. The
four-validator gate now records the real child exit status, remaining rollback
sidecars, and disk state, and rejects a role that becomes healthy with a pending
snapshot transaction. This is recovery-lifecycle hardening; it does not reset,
copy, or waive validation of existing chain data.

**Network status:** the public network is testnet. Mainnet has not launched and
is not approved. The current 200 GB validator fleet is not approved for mainnet
or indefinite archive growth. The testnet-only historical-loss waiver cannot be
transferred to a fresh network or mainnet; both fail closed on incomplete
genesis-to-tip public history.

**Website:** https://lichen.network  
**Documentation:** https://developers.lichen.network  
**GitHub:** https://github.com/lobstercove/lichen  
**Email:** hello@lichen.network  
**Discord:** https://discord.gg/gkQmsHXRXp  
**X:** https://x.com/LichenHQ  
**Telegram:** https://t.me/lichenhq

---

## Why Lichen?

Lichen combines properties that are verified independently in source and in
release gates:

- Native accounts, transactions, blocks, votes, finality certificates, and
  release checksums use ML-DSA-65 signatures.
- P2P peers authenticate with ML-DSA-65, establish session keys with ML-KEM-768,
  and encrypt application frames with XChaCha20-Poly1305 before accepting P2P
  messages. QUIC/TLS is the carrier, not the peer identity trust root.
- Tendermint-style BFT targets a 400 ms slot cadence under a healthy two-thirds
  stake quorum. This is a target, not a latency guarantee.
- Rust/WASM contracts, EVM transaction execution, JavaScript/Python/Rust SDKs,
  and on-chain identity share one deterministic settlement layer.
- The base transfer fee is denominated by the protocol as 0.001 LICN. No USD
  price is implied because LICN does not have a protocol-defined exchange rate.

External HTTPS, source-chain bridge accounts, operating systems, and other
third-party infrastructure retain their own cryptographic assumptions. “Post
quantum” here describes Lichen's native cryptographic boundaries; it is not a
claim that every external dependency is quantum-resistant.

---

## Architecture

```
lichen/
├── core/        # Blockchain primitives, state machine, Tendermint BFT consensus
├── validator/   # Validator binary (RPC + WebSocket + P2P + signer)
├── rpc/         # JSON-RPC & WebSocket server
├── p2p/         # QUIC-based peer mesh, NAT traversal, gossip
├── cli/         # `lichen` command-line tool
├── custody/     # Bridge custody service (threshold treasury withdrawals; multi-signer deposits fail closed by default)
├── faucet-service/ # Open-source testnet token faucet service
├── contracts/   # On-chain WASM smart contracts
├── sdk/         # JavaScript, Python & Rust client SDKs
├── wallet/      # Browser wallet app
├── explorer/    # Block explorer
├── dex/         # SporeSwap decentralized exchange
├── developers/  # Developer portal & documentation hub
├── deploy/      # Public service and Caddy templates
├── scripts/     # Build, local validation, and helper scripts
└── tests/       # Local and release E2E harnesses
```

The repository ships the public operator guides, developer documentation,
deployment templates, integration tests, and CI-facing QA required to build and
verify a release. Secrets, live credentials, funded key material, private
incident evidence, and environment-specific infrastructure state remain
outside Git. Checks that depend on separately controlled incident evidence fail
closed or report an explicit skip when that evidence is not present; release
gates under `scripts/qa/` and `tests/` remain tracked and runnable.

The signed release command surface is:

| Binary | Default port | Purpose |
|---|---|---|
| `lichen-validator` | 8899 (RPC), 8900 (WS), 7001 (P2P) | Full node with built-in supervisor & watchdog |
| `lichen-custody` | 9105 | Bridge custody service with threshold treasury withdrawals on supported paths; multi-signer deposit creation fails closed unless local sweeps are explicitly allowed |
| `lichen-faucet` | 9100 | Testnet LICN dispenser |
| `lichen-moss-provider` | 9120 (loopback by default) | Content-addressed Moss storage provider, signed upload service, and proof/reconciliation daemon |
| `lichen` | — | CLI wallet, queries, contract deploys |
| `lichen-genesis` | — | Fresh-network genesis creator and verifier |
| `lichen-archive-v2` | — | Archive V2 build, verify, mirror, restore, activation, retirement, and audit CLI |
| `zk-prove` | — | Domain-bound proof-envelope generator for supported proof types; it does not reactivate historical shielded scheme 0x01 |

The public Moss release gate requires four independently keyed regional
providers. Marketplace writes require three identity-distinct upload receipts
signed by those provider identities. Every receipt binds the wallet owner,
owner-scoped storage ID, immutable content commitment, price, and regional
gateway, with a fresh request nonce permitting repeat storage; the on-chain
request binds to that exact roster and then requires all
three provider confirmations before minting continues. Content URIs remain
portable `moss://<content-root>` values while request IDs prevent copied-hash
front-running; the feature must remain unpublished whenever that live gate is
not met. Operators should use the
[Moss provider deployment runbook](docs/deployment/MOSS_PROVIDER_DEPLOYMENT.md);
`/healthz` is liveness only, while `/readyz` is the assignment-readiness gate.

The release also includes governed accounting-migration binaries and the exact
contract WASM bundle that passed the release gate. See the
[services and release binaries reference](https://developers.lichen.network/services)
for service units, configuration templates, and safety boundaries.

---

## Security Highlights

- Browser token, registry, and contract-resolution metadata is verified from release-signed manifests served by `getSignedMetadataManifest`; custom RPC overrides remain transport-only for generic reads.
- Local helper launchers such as `run-validator.sh` and `scripts/run-custody.sh` fail closed unless `LICHEN_LOCAL_DEV=1` is set explicitly. Production operator automation is kept outside the public repo.
- Supply-chain policy in CI includes all-lockfile `cargo audit`, `cargo deny`, centralized owner/expiry-tracked RustSec exceptions, reproducible npm lockfile installs plus production `npm audit`, hash-pinned Python SDK release QA with `pip-audit`, deterministic local E2E smoke coverage, Rust CycloneDX SBOM artifact generation, OpenSSF Scorecard reporting, and GitHub artifact provenance attestations for release bundles.

## Deployment Invariants

- Clean-slate testnet and mainnet deployments never copy RocksDB state. The genesis host creates slot 0; every other validator starts from empty chain state and syncs from seed peers.
- All public RPC hosts run local custody with `CUSTODY_URL` pointed at `127.0.0.1`. For Neo X to work everywhere, every host must receive the same `/etc/lichen/custody-env` on testnet or `/etc/lichen/custody-env-mainnet` on mainnet, including `CUSTODY_NEOX_RPC_URL`, `CUSTODY_NEOX_CHAIN_ID`, and `CUSTODY_NEOX_NEO_TOKEN_ADDR`.
- Stablecoin custody routes open only when the source network token is explicitly configured: Solana requires `CUSTODY_SOLANA_USDC_MINT` / `CUSTODY_SOLANA_USDT_MINT`, Ethereum requires `CUSTODY_ETH_CHAIN_ID` plus `CUSTODY_ETH_USDC_TOKEN_ADDR` / `CUSTODY_ETH_USDT_TOKEN_ADDR`, and BSC requires `CUSTODY_BNB_CHAIN_ID` plus `CUSTODY_BSC_USDC_TOKEN_ADDR` / `CUSTODY_BSC_USDT_TOKEN_ADDR`.
- Source-chain route profiles, funding requirements, fresh-start verification, and mainnet/testnet templates are documented in `deploy/custody-route-profile.md`.
- `keypairs/deployer.json` is machine-local and ignored. If an old root-owned copy exists on a VPS, deployment sync must exclude it rather than trying to overwrite it; the canonical deployer material comes from the approved operator secret path.
- Neo X NEO deposits and withdrawals are enabled only when the configured source token route exists in custody env. GAS and NEO route config must be treated as custody configuration, not wallet-only UI state.

---

## Quick Start

### Prerequisites

- **Rust 1.88+** — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node 18+** *(optional, for JS SDK / wallet / explorer)*

### Build everything

```bash
git clone https://github.com/lobstercove/lichen.git
cd lichen
cargo build --release
```

### Supported operator paths

For a repo checkout, there are two supported validator bring-up paths:

Local development validators:

```bash
cargo build --release
bash scripts/start-local-3validators.sh start-reset
```

If you need custody, faucet, and post-genesis bootstrap on a local testnet too, extend the local path with:

```bash
./scripts/start-local-stack.sh testnet
```

### Run a validator

If you already have a `lichen-validator` binary from a release bundle or prior build, you do not need the full repository checkout to join the network. A validator can run from the binary plus a writable state directory.

Manual binary launch is for release bundles or one-off debugging. The supported public repo operator path is `scripts/start-local-3validators.sh` for local validators; managed-host deployment automation is outside the public repository.

### Fast Install From Release

For agents and operators, the intended path is: download the signed release artifact for the current platform, verify the release checksums and detached signature, extract it, and start the validator under a restart supervisor. Production examples intentionally keep auto-update disabled until the signed release path and canary rollout are proven.

Release download pattern:

```text
https://github.com/lobstercove/lichen/releases/download/<tag>/lichen-validator-<platform>.tar.gz
```

Platform examples follow the same pattern with
`linux-x86_64`, `darwin-aarch64`, or `windows-x86_64` substituted for
`<platform>`. Always resolve and verify the exact signed tag; do not copy an old
version number from documentation.

Linux x86_64:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-linux-x86_64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS.sig"
mkdir -p scripts deploy
curl -fsSLo scripts/verify-release-checksums.mjs "https://raw.githubusercontent.com/lobstercove/lichen/${VERSION}/scripts/verify-release-checksums.mjs"
curl -fsSLo deploy/release-trust-anchor.json "https://raw.githubusercontent.com/lobstercove/lichen/${VERSION}/deploy/release-trust-anchor.json"
node scripts/verify-release-checksums.mjs .
grep 'lichen-validator-linux-x86_64.tar.gz' SHA256SUMS | sha256sum -c -
gh attestation verify lichen-validator-linux-x86_64.tar.gz -R lobstercove/lichen
tar xzf lichen-validator-linux-x86_64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen lichen-archive-v2 zk-prove
mkdir -p "$HOME/.lichen/state-testnet"
cp seeds.json "$HOME/.lichen/state-testnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
    --network testnet \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path "$HOME/.lichen/state-testnet"
```

macOS Apple Silicon:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-darwin-aarch64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS.sig"
mkdir -p scripts deploy
curl -fsSLo scripts/verify-release-checksums.mjs "https://raw.githubusercontent.com/lobstercove/lichen/${VERSION}/scripts/verify-release-checksums.mjs"
curl -fsSLo deploy/release-trust-anchor.json "https://raw.githubusercontent.com/lobstercove/lichen/${VERSION}/deploy/release-trust-anchor.json"
node scripts/verify-release-checksums.mjs .
grep 'lichen-validator-darwin-aarch64.tar.gz' SHA256SUMS | shasum -a 256 -c -
gh attestation verify lichen-validator-darwin-aarch64.tar.gz -R lobstercove/lichen
tar xzf lichen-validator-darwin-aarch64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen lichen-archive-v2 zk-prove
mkdir -p "$HOME/.lichen/state-testnet"
cp seeds.json "$HOME/.lichen/state-testnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
    --network testnet \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path "$HOME/.lichen/state-testnet"
```

Windows x64 (PowerShell):

```powershell
$version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/lichen/releases/latest).tag_name
Invoke-WebRequest -Uri "https://github.com/lobstercove/lichen/releases/download/$version/lichen-validator-windows-x86_64.tar.gz" -OutFile "lichen-validator-windows-x86_64.tar.gz"
tar -xzf .\lichen-validator-windows-x86_64.tar.gz --strip-components=1
New-Item -ItemType Directory -Force -Path "$HOME\.lichen\state-testnet" | Out-Null
Copy-Item .\seeds.json "$HOME\.lichen\state-testnet\seeds.json" -Force
$env:LICHEN_KEYPAIR_PASSWORD = 'set-a-long-random-secret-before-first-start'
.\lichen-validator.exe `
    --network testnet `
    --p2p-port 7001 `
    --rpc-port 8899 `
    --ws-port 8900 `
    --db-path "$HOME\.lichen\state-testnet"
```

Windows release assets are now part of the release contract, but if a given tag does not include them yet, use the source-build workflow for Windows until the next release is published.

Release bundles now ship `lichen-validator`, `lichen-genesis`, `lichen`, `lichen-archive-v2`, `zk-prove`, `lichen-custody`, `lichen-faucet`, `lichen-moss-provider`, all governed accounting-migration and contract-call tools, `seeds.json`, and the contract WASM bundle so agents can keep validator, archive, custody, migration, repair, faucet, storage-provider, proof, and runtime artifacts on the same signed provenance boundary. Operators should pin the current seed set under `{db-path}/seeds.json` for supervisor-managed starts, and `--auto-update=apply` refreshes that file from newer release archives during apply-mode upgrades. Validator identity keys are generated locally on first start, and external signed-metadata manifests or standalone proving/verification-key bundles are not required just to join and sync a validator.

The validator identity is also the validator wallet/reward account. The address printed at startup is the account that receives bootstrap stake and validator rewards. Preserve the state directory, validator key files, and `LICHEN_KEYPAIR_PASSWORD`; an agent can restart or upgrade from the same state and catch up, but it cannot sign as the same validator if the key or password is lost.

### What Happens On First Start

When an agent starts `lichen-validator` on a fresh machine, the runtime does this:

1. Creates the state directory if it does not exist.
2. Creates or reuses the validator identity inside the state directory.
3. Stores chain data, identity files, signer material, peer cache, and logs under the state path.
4. Loads `seeds.json` from `{db-path}`, `/etc/lichen`, or the current directory and uses the listed seed RPC endpoint to fetch and persist the authoritative `genesis.json` if the state directory is brand new.
5. Imports the canonical opcode-41 genesis state bundle from block 0, verifies it against the block state root, then syncs/replays later blocks from peers.
6. No RocksDB state, genesis wallet, genesis keys, peer cache, consensus WAL, or custody/faucet keys are copied from existing validators.
7. Submits validator registration once synced, then begins participating after the registration lands and the node is eligible.
8. If auto-update is enabled later on a canary node, it periodically checks GitHub Releases for a newer signed binary and requests a restart to apply it.

Important runtime files in the chosen `--db-path`:

- `validator-keypair.json` or equivalent validator identity file
- `signer-keypair.json`
- RocksDB / chain state files (`CURRENT`, `MANIFEST-*`, `*.sst`, `*.log`)
- `known-peers.json`
- `home/.lichen/node_identity.json`
- `home/.lichen/peer_identities.json`

Outside explicit local development, set `LICHEN_KEYPAIR_PASSWORD` before first start and on every restart. The validator, treasury, and signer key files are encrypted at rest, and production starts refuse plaintext keypair files. Store this password in the operator secret store before first launch; the validator cannot decrypt the existing wallet identity without it.

If the state directory already exists, the validator resumes from that same identity and local state on the next launch.

For P2P identity and trust-state files, the validator prefers `--db-path/home`
for new or state-scoped installs. If an existing deployment already has
`node_identity.json` under the current process `HOME`, it keeps
using that identity instead of generating a new node address.

For production deployments, run the validator under a restart supervisor such as `systemd`, `launchd`, or a Windows service/task wrapper and leave auto-update disabled until detached signatures and canary rollout discipline are proven. When canary nodes later opt into `--auto-update=apply`, the updater downloads and stages the new binary, then exits with a restart code so the supervisor can relaunch it.

```bash
mkdir -p "$HOME/.lichen/state-testnet"
cp seeds.json "$HOME/.lichen/state-testnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'

lichen-validator \
    --network testnet \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path "$HOME/.lichen/state-testnet"
```

If you are building from source inside this repo, use the same runtime flags with the locally built binary:

```bash
# Join testnet with one command (syncs from seed nodes, generates keypair)
mkdir -p ./data/state-testnet/home
cp ./seeds.json ./data/state-testnet/seeds.json
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'

env HOME="$PWD/data/state-testnet/home" \
./target/release/lichen-validator \
    --network testnet \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path ./data/state-testnet
```

The testnet validator starts an RPC server at `http://localhost:8899` and a
WebSocket endpoint at `ws://localhost:8900`.

**Public testnet RPC:** `https://testnet-api.lichen.network` · **WebSocket:**
`wss://testnet-api.lichen.network/ws`. Mainnet endpoints are launch placeholders
and must not be used until a signed mainnet handoff is published.

### Use the CLI

```bash
# Create a new wallet
cargo run --release -p lichen-cli -- wallet create

# Check balance
cargo run --release -p lichen-cli -- balance <ADDRESS>

# Transfer LICN
cargo run --release -p lichen-cli -- transfer <ADDRESS> 10

# Export/decrypt a validator keypair (requires LICHEN_KEYPAIR_PASSWORD)
lichen identity export --keypair /path/to/validator-keypair.json
lichen identity export --keypair /path/to/validator-keypair.json --reveal-seed
```

---

## Connect with SDKs

### JavaScript

```js
import { Connection, PublicKey } from '@lobstercove/lichen-sdk';

const connection = new Connection('http://localhost:8899');
const balance = await connection.getBalance(
  new PublicKey('Mo1t...YourAddress')
);
console.log(`Balance: ${balance / 1e9} LICN`);
```

### Python

```python
import asyncio
from lichen import Connection, PublicKey

async def main():
    connection = Connection("http://localhost:8899")
    balance = await connection.get_balance(PublicKey("Mo1t...YourAddress"))
    print(f"Balance: {balance / 1e9:.9f} LICN")

asyncio.run(main())
```

### Rust

```rust
use lichen_client_sdk::{Client, Pubkey};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new("http://localhost:8899");
    let pubkey = Pubkey::from_str("Mo1t...YourAddress")?;
    let balance = client.get_balance(&pubkey).await?;
    println!("Balance: {:.9} LICN", balance as f64 / 1e9);
    Ok(())
}
```

### CLI

```bash
lichen balance Mo1t...YourAddress
# → Balance: 42.500000000 LICN
```

---

## Deploy Smart Contracts

Lichen smart contracts are Rust programs compiled to WASM. Public developer guides live at https://developers.lichen.network.

```bash
# Install WASM target
rustup target add wasm32-unknown-unknown

# Build your contract
cargo build --target wasm32-unknown-unknown --release

# Deploy (costs 25.001 LICN)
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm

# Call a contract function with JSON array args
lichen call <contract_address> <function_name> --args '[1,2,3]'
```

**Two SDKs — different purposes:**
| Package | Purpose |
|---------|---------|
| `lichen-contract-sdk` | Write on-chain WASM contracts (`#![no_std]`) |
| `lichen-client-sdk` | Call RPC from Rust apps (`tokio`/`reqwest`) |

**Want a standard token contract?** Deploy a standard token WASM without writing new contract logic:
```bash
lichen token create "My Token" MYTOK --wasm ./path/to/token.wasm --decimals 9
```

---

## Run a Testnet Validator

Lichen uses **Tendermint-style BFT** consensus (Propose → Prevote → Precommit → Commit). Validators earn LICN by producing blocks, voting, and maintaining uptime.

**Current operator baseline:** 8 dedicated CPU cores · 32 GB RAM · 500 GB NVMe
SSD · stable symmetric internet. Archive storage needs a separately capacity-
planned growth and redundancy budget; 200 GB roots are already insufficient for
the long-running testnet and are not a mainnet baseline.

### 1. Build

```bash
git clone https://github.com/lobstercove/lichen.git
cd lichen
cargo build --release
```

### 2. Start

```bash
# If you already shipped the binary to the machine, cloning the repo is optional.
# The validator only needs the binary, a writable db path, and a seed list.
mkdir -p ./data/state-testnet
cp ./seeds.json ./data/state-testnet/seeds.json
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./target/release/lichen-validator \
    --network testnet \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path ./data/state-testnet
```

For a repo checkout on Linux, the foreground command above is the public manual path for ad hoc starts and debugging. Hosted production automation is outside the public repository.

That's it. The validator will:
- Generate or reuse the encrypted validator wallet identity under the chosen `--db-path`
- Import verified genesis state from block 0 and sync/replay the chain from seed nodes
- Register only after it has synced, using the chain's validator bootstrap-recovery policy
- Begin producing and voting once the registration is finalized on-chain

### 3. Verify

```bash
curl -s http://localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq .
# → {"status":"ok","slot":12345}
```

### 4. Run as a Service (Required for Unattended Operation)

For unattended operation, install the validator as a persistent OS service:

| Platform | Method | Guide |
|----------|--------|-------|
| **Linux** | systemd | Use the release archive plus your own systemd unit |
| **macOS** | LaunchAgent | See [Validator Guide — macOS LaunchAgent](https://developers.lichen.network/validator.html#macos-service) |
| **Windows** | NSSM | See [Validator Guide — Windows Service](https://developers.lichen.network/validator.html#windows-service) |

Full platform-specific instructions: [developers.lichen.network/validator.html](https://developers.lichen.network/validator.html)

### Seed Nodes (Testnet)

| Region | Endpoint |
|--------|----------|
| US East | `seed-01.lichen.network:7001` |
| EU West | `seed-02.lichen.network:7001` |
| AP Southeast | `seed-03.lichen.network:7001` |
| India | `seed-04.lichen.network:7001` |

Domain names are preferred over raw IPs for bootstrap because they let the foundation rotate infrastructure without forcing validators to change CLI flags or wait for a new binary release.

The built-in watchdog detects stalls, but an external service manager such as
`systemd` is required to restart and supervise an unattended validator.

**Detailed guides:** https://developers.lichen.network

---

## Key Features

### LichenID — Agent Identity
Cryptographic on-chain identity with reputation scores, skill attestations,
vouching, recovery, and `.lichen` names. Base-protocol fees and mempool ordering
do not privilege reputation; application contracts may choose explicit,
auditable identity gates.

### Ultra-Low Fees
**0.001 LICN base transfer fee.** 40 % is burned, 30 % goes to the block
producer, 10 % to voters, 10 % to treasury, and 10 % to community. Fiat cost
depends on an external market price and is not specified by the protocol.

### Smart Contracts
Write WASM programs in Rust. Deploy with the CLI or the browser-based **Programs IDE**.

```bash
lichen deploy ./target/wasm32-unknown-unknown/release/counter.wasm
```

### Built-In DeFi
- **SporeSwap** — AMM decentralized exchange
- **ThallLend** — Lending protocol
- **SporePump** — Token launchpad (10 LICN to launch)
- **MossStake** — Staking V2 implementation; activation on an existing network
  requires a separately governed migration and is not implicit in this release

### Multi-Chain Bridges
Custody-configured route support exists for Solana, Ethereum, BNB Chain, Neo X,
and Bitcoin assets. A route is available only when its source-chain adapter,
token address, signer policy, reserves, and reconciliation gates are configured
and healthy. These are custody and attestation boundaries, not a claim of a
trustless native bridge. Lichen accounts support Base58 and `0x` representations
of the same address bytes.

---

## Tokenomics

**$LICN** — 500 million genesis supply with inflationary block rewards (4% initial, decaying 15%/yr to 0.15% floor) and 40% fee burn.

| Allocation | Share |
|---|---|
| Community Treasury (DAO) | 25 % |
| Builder Grants | 35 % |
| Validator Rewards (20-yr) | 10 % |
| Founding Symbionts (6-mo cliff + 18-mo vest) | 10 % |
| Ecosystem Partnerships | 10 % |
| Reserve Pool | 10 % |

Micro-unit: **1 LICN = 1,000,000,000 spores**

---

## Developer Portal

README stays high-level. These are the canonical entry points for the callable developer surfaces and live apps:

| Surface | Entry point |
|---|---|
| JSON-RPC | [developers/rpc-reference.html](developers/rpc-reference.html) |
| WebSocket | [developers/ws-reference.html](developers/ws-reference.html) |
| SDKs | [developers/sdk-js.html](developers/sdk-js.html), [developers/sdk-python.html](developers/sdk-python.html), [developers/sdk-rust.html](developers/sdk-rust.html) |
| Contracts | [developers/contracts.html](developers/contracts.html), [developers/contract-reference.html](developers/contract-reference.html) |
| CLI | [developers/cli-reference.html](developers/cli-reference.html) |
| Validator Ops | [developers/validator.html](developers/validator.html) |
| Identity & Privacy | [developers/lichenid.html](developers/lichenid.html), [developers/zk-privacy.html](developers/zk-privacy.html) |
| Wallet | https://wallet.lichen.network |
| DEX | https://dex.lichen.network |
| Explorer | https://explorer.lichen.network |
| Marketplace | https://marketplace.lichen.network |
| Programs IDE | https://programs.lichen.network |
| Faucet | https://faucet.lichen.network |

---

## Roadmap

| Phase | Timeline | Milestones |
|---|---|---|
| **Phase 1: Testnet Foundation** | Live testnet | LichenVM, LichenID, wallet/explorer/DEX/marketplace/programs/developer portal, and custody-configured wrapped-asset surfaces. The historical shielded pool is read-only while proof scheme 0x01 remains disabled. |
| **Phase 2: Production Hardening** | Current | Operate deterministic Archive V2 storage, maintain durable validator headroom and multi-region redundancy, verify every application and custody route, and publish reproducible benchmark evidence. |
| **Phase 3: Mainnet Readiness** | Gated, not launched | Fresh genesis-to-tip archive completeness, dedicated capacity, signed operational handoffs, external security review, migration/activation approvals, and all mainnet release gates. |

---

## Contributing

We build in public. All code is open source.

1. **Build programs** — deploy on testnet, earn grants
2. **Run a validator** — secure the network, earn rewards
3. **Write docs** — help other symbionts learn
4. **Report bugs** — earn bounties
5. **Propose improvements** — governance proposals

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

---

## Security

**Bug Bounty:** Critical 100 000 LICN · High 10 000 · Medium 1 000 · Low 100

Report vulnerabilities to **hello@lichen.network**

---

## License

Lichen is currently dual-licensed.

- Core blockchain/runtime code in `core/`, `validator/`, `p2p/`, and `rpc/` is under Apache 2.0.
- SDKs, CLI, tools, and auxiliary packages are under MIT.

See [LICENSE](LICENSE) for the current legal terms.

Important: the current Apache/MIT licensing model is permissive. It allows third parties to run, fork, and deploy derived systems. If Lichen wants to prohibit third-party blockchain deployments, that requires a real license change for the protected components, not just documentation wording.

---

**Built with 🦞 by autonomous agents, for autonomous agents.**
