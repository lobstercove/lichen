# Lichen 🦞⚡

**The first blockchain built by agents, for agents.**

Ultra-low fees · Sub-second BFT block commitment · Agent-native identity · Multi-language SDKs

[![License: Apache--2.0%20%2B%20MIT](https://img.shields.io/badge/License-Apache--2.0%20%2B%20MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.88+-00C9DB.svg)](https://www.rust-lang.org)

**Release-ready status:** main validated and published at tag `v0.4.0`.

**Website:** https://lichen.network  
**Documentation:** https://developers.lichen.network  
**GitHub:** https://github.com/lobstercove/lichen  
**Email:** hello@lichen.network  
**Discord:** https://discord.gg/gkQmsHXRXp  
**X:** https://x.com/LichenHQ  
**Telegram:** https://t.me/lichenhq

---

## Why Lichen?

Current blockchains charge agents hundreds of dollars a year just to exist on-chain. Lichen fixes that:

| | Lichen | Solana | Ethereum |
|---|---|---|---|
| **Tx cost** | $0.0001 | $0.00025 | $1–50 |
| **Commit latency** | ~400 ms typical block commitment | 400 ms | 12 s |
| **Agent identity** | Built-in (LichenID) | None | None |
| **Smart-contract langs** | Rust (WASM); JS, Python, Rust (SDKs) | Rust | Solidity |

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
├── deploy/      # Systemd services, Caddy configs
├── infra/       # Docker Compose, Prometheus, Grafana
├── scripts/     # Operational scripts (genesis, health-check, deploy)
└── tests/       # End-to-end integration tests
```

Four binaries ship from this repo:

| Binary | Default port | Purpose |
|---|---|---|
| `lichen-validator` | 8899 (RPC), 8900 (WS), 7001 (P2P) | Full node with built-in supervisor & watchdog |
| `lichen-custody` | 9105 | Bridge custody service with threshold treasury withdrawals on supported paths; multi-signer deposit creation fails closed unless local sweeps are explicitly allowed |
| `lichen-faucet` | 9100 | Testnet LICN dispenser |
| `lichen` | — | CLI wallet, queries, contract deploys |

---

## Security Highlights

- Browser token, registry, and contract-resolution metadata is verified from release-signed manifests served by `getSignedMetadataManifest`; custom RPC overrides remain transport-only for generic reads.
- Local helper launchers such as `run-validator.sh` and `scripts/run-custody.sh` fail closed unless `LICHEN_LOCAL_DEV=1` is set explicitly. Production setup stays on `deploy/setup.sh` plus systemd units.
- Supply-chain policy in CI includes `cargo audit`, `cargo deny`, and Rust CycloneDX SBOM artifact generation for the workspace.

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

### Run a validator

If you already have a `lichen-validator` binary from a release bundle or prior build, you do not need the full repository checkout to join the network. A validator can run from the binary plus a writable state directory.

### Fast Install From Release

For agents and operators, the intended path is: download the signed release artifact for the current platform, verify the release checksums and detached signature, extract it, and start the validator under a restart supervisor. Production examples intentionally keep auto-update disabled until the signed release path and canary rollout are proven.

Release download pattern:

```text
https://github.com/lobstercove/lichen/releases/download/<tag>/lichen-validator-<platform>.tar.gz
```

Examples:
- `https://github.com/lobstercove/lichen/releases/download/v0.1.0/lichen-validator-linux-x86_64.tar.gz`
- `https://github.com/lobstercove/lichen/releases/download/v0.1.0/lichen-validator-darwin-aarch64.tar.gz`
- `https://github.com/lobstercove/lichen/releases/download/v0.1.0/lichen-validator-windows-x86_64.tar.gz`

Linux x86_64:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-linux-x86_64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-linux-x86_64.tar.gz' SHA256SUMS | sha256sum -c -
tar xzf lichen-validator-linux-x86_64.tar.gz --strip-components=1
chmod +x lichen-validator
mkdir -p "$HOME/.lichen/state-mainnet"
./lichen-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.lichen/state-mainnet" \
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

macOS Apple Silicon:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-darwin-aarch64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-darwin-aarch64.tar.gz' SHA256SUMS | shasum -a 256 -c -
tar xzf lichen-validator-darwin-aarch64.tar.gz --strip-components=1
chmod +x lichen-validator
mkdir -p "$HOME/.lichen/state-mainnet"
./lichen-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.lichen/state-mainnet" \
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

Windows x64 (PowerShell):

```powershell
$version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/lichen/releases/latest).tag_name
Invoke-WebRequest -Uri "https://github.com/lobstercove/lichen/releases/download/$version/lichen-validator-windows-x86_64.tar.gz" -OutFile "lichen-validator-windows-x86_64.tar.gz"
tar -xzf .\lichen-validator-windows-x86_64.tar.gz --strip-components=1
New-Item -ItemType Directory -Force -Path "$HOME\.lichen\state-mainnet" | Out-Null
.\lichen-validator.exe `
    --network mainnet `
    --p2p-port 8001 `
    --rpc-port 9899 `
    --ws-port 9900 `
    --db-path "$HOME\.lichen\state-mainnet" `
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

Windows release assets are now part of the release contract, but if a given tag does not include them yet, use the source-build workflow for Windows until the next release is published.

### What Happens On First Start

When an agent starts `lichen-validator` on a fresh machine, the runtime does this:

1. Creates the state directory if it does not exist.
2. Creates or reuses the validator identity inside the state directory.
3. Stores chain data, identity files, signer material, peer cache, and logs under the state path.
4. Connects to the bootstrap peers (`seed-01.lichen.network`, `seed-02.lichen.network`, `seed-03.lichen.network`).
5. Syncs state from the network.
6. Begins participating as a validator once synced and eligible.
7. If auto-update is enabled later on a canary node, it periodically checks GitHub Releases for a newer signed binary and requests a restart to apply it.

Important runtime files in the chosen `--db-path`:

- `validator-keypair.json` or equivalent validator identity file
- `signer-keypair.json`
- RocksDB / chain state files (`CURRENT`, `MANIFEST-*`, `*.sst`, `*.log`)
- `known-peers.json`
- `home/.lichen/node_identity.json`
- `home/.lichen/peer_identities.json`

If the state directory already exists, the validator resumes from that same identity and local state on the next launch.

For P2P identity and trust-state files, the validator prefers `--db-path/home`
for new or state-scoped installs. If an existing deployment already has
`node_identity.json` under the current process `HOME`, it keeps
using that identity instead of generating a new node address.

For production deployments, run the validator under a restart supervisor such as `systemd`, `launchd`, or a Windows service/task wrapper and leave auto-update disabled until detached signatures and canary rollout discipline are proven. When canary nodes later opt into `--auto-update=apply`, the updater downloads and stages the new binary, then exits with a restart code so the supervisor can relaunch it.

```bash
mkdir -p "$HOME/.lichen/state-mainnet"

lichen-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.lichen/state-mainnet" \
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

If you are building from source inside this repo, use the same runtime flags with the locally built binary:

```bash
# Join mainnet with one command (syncs from seed nodes, generates keypair)
mkdir -p ./data/state-mainnet/home

env HOME="$PWD/data/state-mainnet/home" \
./target/release/lichen-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path ./data/state-mainnet \
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

The validator starts an RPC server at `http://localhost:9899` and a WebSocket endpoint at `ws://localhost:9900`.

**Mainnet RPC:** `https://rpc.lichen.network` · **WebSocket:** `wss://rpc.lichen.network/ws`

### Use the CLI

```bash
# Create a new wallet
cargo run --release -p lichen-cli -- wallet new

# Check balance
cargo run --release -p lichen-cli -- balance <ADDRESS>

# Transfer LICN
cargo run --release -p lichen-cli -- transfer --to <ADDRESS> --amount 10
```

---

## Connect with SDKs

### JavaScript

```js
import { Connection, PublicKey } from '@lichen/sdk';

const connection = new Connection('http://localhost:8899');
const balance = await connection.getBalance(
  new PublicKey('Mo1t...YourAddress')
);
console.log(`Balance: ${balance / 1e9} LICN`);
```

### Python

```python
from lichen import Client, PublicKey

client = Client("http://localhost:8899")
balance = client.get_balance(PublicKey("Mo1t...YourAddress"))
print(f"Balance: {balance / 1e9:.9f} LICN")
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

Lichen smart contracts are Rust programs compiled to WASM. See `docs/guides/CONTRACT_DEVELOPMENT.md` for the full guide.

```bash
# Install WASM target
rustup target add wasm32-unknown-unknown

# Build your contract
cargo build --target wasm32-unknown-unknown --release

# Deploy (costs 25.001 LICN)
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm

# Call a contract function
lichen call <contract_address> <function_name> [args]
```

**Two SDKs — different purposes:**
| Package | Purpose |
|---------|---------|
| `lichen-contract-sdk` | Write on-chain WASM contracts (`#![no_std]`) |
| `lichen-client-sdk` | Call RPC from Rust apps (`tokio`/`reqwest`) |

**Don't need custom logic?** Create a standard token without writing code:
```bash
lichen token create "My Token" MYTOK --supply 1000000 --decimals 9
```

---

## Run a Mainnet Validator

Lichen uses **Tendermint-style BFT** consensus (Propose → Prevote → Precommit → Commit). Validators earn LICN by producing blocks, voting, and maintaining uptime.

**Minimum requirements:** 2 CPU cores · 2 GB RAM · 50 GB SSD · stable internet

### 1. Build

```bash
git clone https://github.com/lobstercove/lichen.git
cd lichen
cargo build --release
```

### 2. Start

```bash
# If you already shipped the binary to the machine, cloning the repo is optional.
# The validator only needs the binary, a writable db path, and bootstrap peers.
./target/release/lichen-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path ./data/state-mainnet \
    --bootstrap-peers seed-01.lichen.network:8001,seed-02.lichen.network:8001,seed-03.lichen.network:8001
```

That's it. The validator will:
- Generate a keypair (saved to `~/.lichen/validators/validator-mainnet.json`)
- Sync the chain from seed nodes
- Receive a 100K LICN bootstrap stake grant (first 200 validators only)
- Begin producing & voting on blocks

### 3. Verify

```bash
curl -s http://localhost:9899 -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq .
# → {"status":"ok","slot":12345}
```

### 4. Run as a Service (Optional)

For unattended operation, install the validator as a persistent OS service:

| Platform | Method | Guide |
|----------|--------|-------|
| **Linux** | systemd | `sudo bash deploy/setup.sh mainnet` — creates unit, user, env file |
| **macOS** | LaunchAgent | See [Validator Guide — macOS LaunchAgent](https://developers.lichen.network/validator.html#macos-service) |
| **Windows** | NSSM | See [Validator Guide — Windows Service](https://developers.lichen.network/validator.html#windows-service) |

Full platform-specific instructions: [developers.lichen.network/validator.html](https://developers.lichen.network/validator.html)

### Seed Nodes (Mainnet)

| Region | Endpoint |
|--------|----------|
| US East | `seed-01.lichen.network:8001` |
| EU West | `seed-02.lichen.network:8001` |
| AP Southeast | `seed-03.lichen.network:8001` |

Domain names are preferred over raw IPs for bootstrap because they let the foundation rotate infrastructure without forcing validators to change CLI flags or wait for a new binary release.

The built-in **supervisor** auto-restarts on crash and the **watchdog** alerts on stall — no external process manager needed.

**Detailed guides:**
- [Validator Setup](docs/consensus/VALIDATOR_SETUP.md)
- [Production Deployment](docs/deployment/PRODUCTION_DEPLOYMENT.md)
- [Custody Deployment](docs/deployment/CUSTODY_DEPLOYMENT.md)
- [SKILL.md](SKILL.md) — Full agent reference (contracts, RPC, identity, staking)

---

## Key Features

### LichenID — Agent Identity
Cryptographic on-chain identity with reputation tiers, skill attestations, and fee discounts. Agents build trust through verifiable contribution history.

### Ultra-Low Fees
**$0.0001 per transaction (0.001 LICN).** 40 % burned (counter-pressure to inflation), 30 % to block producer, 10 % to voters, 10 % to treasury, 10 % to community.

### Smart Contracts
Write WASM programs in Rust. Deploy with the CLI or the browser-based **Programs IDE**.

```bash
lichen deploy --program ./target/wasm32-unknown-unknown/release/counter.wasm
```

### Built-In DeFi
- **SporeSwap** — AMM decentralized exchange
- **ThallLend** — Lending protocol
- **SporePump** — Token launchpad (0.1 LICN to launch)
- **MossStake** — Liquid staking

### Multi-Chain Bridges
Native bridge support for Solana, Ethereum and BNB Chain assets (wSOL, wETH, wBNB). Dual address format — Base58 *and* 0x hex on the same account.

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

Canonical developer-facing API docs:

- **[JSON-RPC API](developers/rpc-reference.html)** — Canonical RPC method index
- **[WebSocket API](developers/ws-reference.html)** — Canonical WS subscriptions index
- **[Detailed RPC Guide](docs/guides/RPC_API_REFERENCE.md)** — Full request/response examples
- **[Validator Setup](docs/consensus/VALIDATOR_SETUP.md)** — Runtime validator operations baseline

---

## Roadmap

| Phase | Timeline | Milestones |
|---|---|---|
| **Phase 1: Live Foundation** | Live now | Mainnet + testnet live, LichenVM, LichenID, shielded pool + transparent STARK privacy, wallet/explorer/DEX/marketplace/programs/developer portal, Solana + Ethereum + BNB wrapped asset support |
| **Phase 2: Network Expansion** | Current buildout | Validator growth and hardening, better bridge + custody UX, deeper SporeSwap and wrapped-asset liquidity, faster SDK and validator onboarding, broader app launches across payments, AI agents, identity, and compute |
| **Phase 3: Ecosystem Scale** | Next | Larger validator footprint, deeper cross-chain liquidity and routing, stronger privacy and coordination tooling, full-stack agent economy across DeFi/payments/compute/marketplaces, institutional-grade reliability |

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
