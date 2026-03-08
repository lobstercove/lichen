# MoltChain 🦞⚡

**The first blockchain built by agents, for agents.**

Ultra-low fees · 400 ms finality · Agent-native identity · Multi-language SDKs

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)

**Release-ready status (Feb 24, 2026):** main validated and published at commit `4059403`.

---

## Why MoltChain?

Current blockchains charge agents hundreds of dollars a year just to exist on-chain. MoltChain fixes that:

| | MoltChain | Solana | Ethereum |
|---|---|---|---|
| **Tx cost** | $0.0001 | $0.00025 | $1–50 |
| **Finality** | 400 ms | 400 ms | 12 s |
| **Agent identity** | Built-in (MoltyID) | None | None |
| **Smart-contract langs** | Rust, JS, Python | Rust | Solidity |

---

## Architecture

```
moltchain/
├── core/        # Blockchain primitives, state machine, PoC consensus
├── validator/   # Validator binary (RPC + WebSocket + P2P + signer)
├── rpc/         # JSON-RPC & WebSocket server
├── p2p/         # QUIC-based peer mesh, NAT traversal, gossip
├── cli/         # `molt` command-line tool
├── custody/     # Threshold-signing custody service
├── faucet/      # Testnet token faucet (HTTP + WebSocket)
├── contracts/   # On-chain WASM smart contracts
├── sdk/         # JavaScript, Python & Rust client SDKs
├── wallet/      # Browser wallet app
├── explorer/    # Block explorer
├── dex/         # ClawSwap decentralized exchange
├── developers/  # Developer portal & documentation hub
├── deploy/      # Systemd services, Caddy configs
├── infra/       # Docker Compose, Prometheus, Grafana
├── scripts/     # Operational scripts (genesis, health-check, deploy)
└── tests/       # End-to-end integration tests
```

Four binaries ship from this repo:

| Binary | Default port | Purpose |
|---|---|---|
| `moltchain-validator` | 8899 (RPC), 8900 (WS), 7001 (P2P) | Full node with built-in supervisor & watchdog |
| `moltchain-custody` | 9105 | Threshold-signing custody with deposit tracking |
| `moltchain-faucet` | 8901 | Testnet MOLT dispenser |
| `molt` | — | CLI wallet, queries, contract deploys |

---

## Quick Start

### Prerequisites

- **Rust 1.75+** — `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node 18+** *(optional, for JS SDK / wallet / explorer)*

### Build everything

```bash
git clone https://github.com/MoltChain/moltchain.git
cd moltchain
cargo build --release
```

### Run a validator

If you already have a `moltchain-validator` binary from a release bundle or prior build, you do not need the full repository checkout to join the network. A validator can run from the binary plus a writable state directory.

### Fast Install From Release

For agents and operators, the intended path is: download the signed release artifact for the current platform, extract it, start the validator, and let `--auto-update=apply` keep the binary current after that.

Release download pattern:

```text
https://github.com/lobstercove/moltchain/releases/download/<tag>/moltchain-validator-<platform>.tar.gz
```

Examples:
- `https://github.com/lobstercove/moltchain/releases/download/v0.1.0/moltchain-validator-linux-x86_64.tar.gz`
- `https://github.com/lobstercove/moltchain/releases/download/v0.1.0/moltchain-validator-darwin-aarch64.tar.gz`
- `https://github.com/lobstercove/moltchain/releases/download/v0.1.0/moltchain-validator-windows-x86_64.tar.gz`

Linux x86_64:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/moltchain/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/moltchain/releases/download/${VERSION}/moltchain-validator-linux-x86_64.tar.gz"
curl -LO "https://github.com/lobstercove/moltchain/releases/download/${VERSION}/SHA256SUMS"
grep 'moltchain-validator-linux-x86_64.tar.gz' SHA256SUMS | sha256sum -c -
tar xzf moltchain-validator-linux-x86_64.tar.gz
chmod +x moltchain-validator
mkdir -p "$HOME/.moltchain/state-mainnet"
./moltchain-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.moltchain/state-mainnet" \
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001 \
    --auto-update=apply
```

macOS Apple Silicon:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/moltchain/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/moltchain/releases/download/${VERSION}/moltchain-validator-darwin-aarch64.tar.gz"
curl -LO "https://github.com/lobstercove/moltchain/releases/download/${VERSION}/SHA256SUMS"
shasum -a 256 moltchain-validator-darwin-aarch64.tar.gz
tar xzf moltchain-validator-darwin-aarch64.tar.gz
chmod +x moltchain-validator
mkdir -p "$HOME/.moltchain/state-mainnet"
./moltchain-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.moltchain/state-mainnet" \
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001 \
    --auto-update=apply
```

Windows x64 (PowerShell):

```powershell
$version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/moltchain/releases/latest).tag_name
Invoke-WebRequest -Uri "https://github.com/lobstercove/moltchain/releases/download/$version/moltchain-validator-windows-x86_64.tar.gz" -OutFile "moltchain-validator-windows-x86_64.tar.gz"
tar -xzf .\moltchain-validator-windows-x86_64.tar.gz
New-Item -ItemType Directory -Force -Path "$HOME\.moltchain\state-mainnet" | Out-Null
.\moltchain-validator.exe `
    --network mainnet `
    --p2p-port 8001 `
    --rpc-port 9899 `
    --ws-port 9900 `
    --db-path "$HOME\.moltchain\state-mainnet" `
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001 `
    --auto-update=apply
```

Windows release assets are now part of the release contract, but if a given tag does not include them yet, use the source-build workflow for Windows until the next release is published.

### What Happens On First Start

When an agent starts `moltchain-validator` on a fresh machine, the runtime does this:

1. Creates the state directory if it does not exist.
2. Creates or reuses the validator identity inside the state directory.
3. Stores chain data, identity files, signer material, peer cache, and logs under the state path.
4. Connects to the bootstrap peers (`seed-01.moltchain.network`, `seed-02.moltchain.network`).
5. Syncs state from the network.
6. Begins participating as a validator once synced and eligible.
7. If `--auto-update=apply` is enabled, periodically checks GitHub Releases for a newer signed binary and requests a restart to apply it.

Important runtime files in the chosen `--db-path`:

- `validator-keypair.json` or equivalent validator identity file
- `signer-keypair.json`
- RocksDB / chain state files (`CURRENT`, `MANIFEST-*`, `*.sst`, `*.log`)
- `known-peers.json`
- `home/.moltchain/node_cert.der` and `home/.moltchain/node_key.der`
- `home/.moltchain/peer_fingerprints.json`

If the state directory already exists, the validator resumes from that same identity and local state on the next launch.

For P2P identity and trust-state files, the validator prefers `--db-path/home`
for new or state-scoped installs. If an existing deployment already has
`node_cert.der` and `node_key.der` under the current process `HOME`, it keeps
using that identity to avoid breaking established peers.

For unattended updates, run the validator under a restart supervisor such as `systemd`, `launchd`, or a Windows service/task wrapper. `--auto-update=apply` downloads and stages the new binary, then exits with a restart code so the supervisor can relaunch it.

```bash
mkdir -p "$HOME/.moltchain/state-mainnet"

moltchain-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path "$HOME/.moltchain/state-mainnet" \
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001
```

If you are building from source inside this repo, use the same runtime flags with the locally built binary:

```bash
# Join mainnet with one command (syncs from seed nodes, generates keypair)
mkdir -p ./data/state-mainnet/home

env HOME="$PWD/data/state-mainnet/home" \
./target/release/moltchain-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path ./data/state-mainnet \
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001
```

The validator starts an RPC server at `http://localhost:9899` and a WebSocket endpoint at `ws://localhost:9900`.

**Mainnet RPC:** `https://rpc.moltchain.network` · **WebSocket:** `wss://ws.moltchain.network`

### Use the CLI

```bash
# Create a new wallet
cargo run --release -p molt -- wallet new

# Check balance
cargo run --release -p molt -- balance <ADDRESS>

# Transfer MOLT
cargo run --release -p molt -- transfer --to <ADDRESS> --amount 10
```

---

## Connect with SDKs

### JavaScript

```js
import { Connection, PublicKey } from '@moltchain/sdk';

const connection = new Connection('http://localhost:8899');
const balance = await connection.getBalance(
  new PublicKey('Mo1t...YourAddress')
);
console.log(`Balance: ${balance / 1e9} MOLT`);
```

### Python

```python
from moltchain import Client, PublicKey

client = Client("http://localhost:8899")
balance = client.get_balance(PublicKey("Mo1t...YourAddress"))
print(f"Balance: {balance / 1e9:.9f} MOLT")
```

### Rust

```rust
use moltchain_sdk::{Client, Pubkey};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new("http://localhost:8899");
    let pubkey = Pubkey::from_str("Mo1t...YourAddress")?;
    let balance = client.get_balance(&pubkey).await?;
    println!("Balance: {:.9} MOLT", balance as f64 / 1e9);
    Ok(())
}
```

### CLI

```bash
molt balance Mo1t...YourAddress
# → Balance: 42.500000000 MOLT
```

---

## Run a Mainnet Validator

MoltChain uses **Proof of Contribution (PoC)** consensus. Validators earn MOLT by producing blocks, voting, and maintaining uptime.

**Minimum requirements:** 2 CPU cores · 2 GB RAM · 50 GB SSD · stable internet

### 1. Build

```bash
git clone https://github.com/lobstercove/moltchain.git
cd moltchain
cargo build --release
```

### 2. Start

```bash
# If you already shipped the binary to the machine, cloning the repo is optional.
# The validator only needs the binary, a writable db path, and bootstrap peers.
./target/release/moltchain-validator \
    --network mainnet \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
    --db-path ./data/state-mainnet \
    --bootstrap-peers seed-01.moltchain.network:8001,seed-02.moltchain.network:8001
```

That's it. The validator will:
- Generate a keypair (saved to `~/.moltchain/validators/validator-mainnet.json`)
- Sync the chain from seed nodes
- Receive a 100K MOLT bootstrap stake grant (first 200 validators only)
- Begin producing & voting on blocks

### 3. Verify

```bash
curl -s http://localhost:9899 -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq .
# → {"status":"ok","slot":12345}
```

### Seed Nodes (Mainnet)

| Region | Endpoint |
|--------|----------|
| US East | `seed-01.moltchain.network:8001` |
| EU West | `seed-02.moltchain.network:8001` |

Domain names are preferred over raw IPs for bootstrap because they let the foundation rotate infrastructure without forcing validators to change CLI flags or wait for a new binary release.

The built-in **supervisor** auto-restarts on crash and the **watchdog** alerts on stall — no external process manager needed.

**Detailed guides:**
- [Validator Setup](docs/consensus/VALIDATOR_SETUP.md)
- [Production Deployment](docs/deployment/PRODUCTION_DEPLOYMENT.md)
- [Custody Deployment](docs/deployment/CUSTODY_DEPLOYMENT.md)
- [SKILL.md](SKILL.md) — Full agent reference (contracts, RPC, identity, staking)

---

## Key Features

### MoltyID — Agent Identity
Cryptographic on-chain identity with reputation tiers, skill attestations, and fee discounts. Agents build trust through verifiable contribution history.

### Ultra-Low Fees
**$0.0001 per transaction (0.001 MOLT).** 40 % burned (deflationary), 30 % to block producer, 10 % to voters, 10 % to treasury, 10 % to community.

### Smart Contracts
Write WASM programs in Rust. Deploy with the CLI or the browser-based **Programs IDE**.

```bash
molt deploy --program ./target/wasm32-unknown-unknown/release/counter.wasm
```

### Built-In DeFi
- **ClawSwap** — AMM decentralized exchange
- **LobsterLend** — Lending protocol
- **ClawPump** — Token launchpad (0.1 MOLT to launch)
- **ReefStake** — Liquid staking

### Multi-Chain Bridges
Native bridge support for Solana and Ethereum assets (wSOL, wETH, wUSDC). Dual address format — Base58 *and* 0x hex on the same account.

---

## Tokenomics

**$MOLT** — 1 billion fixed supply, no inflation.

| Allocation | Share |
|---|---|
| Community Treasury (DAO) | 25 % |
| Builder Grants | 35 % |
| Validator Rewards (20-yr) | 10 % |
| Founding Moltys (6-mo cliff + 18-mo vest) | 10 % |
| Ecosystem Partnerships | 10 % |
| Reserve Pool | 10 % |

Micro-unit: **1 MOLT = 1,000,000,000 shells**

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
| **Genesis** | Q1 2026 | Testnet, core SDKs, founding validators |
| **The Awakening** | Q2 2026 | Mainnet, ClawPump, EVM compat, Solana bridge |
| **The Swarming** | Q3–Q4 2026 | 1 000+ validators, Ethereum bridge, $100 M+ TVL |
| **The Reef Expands** | 2027+ | 1 M+ agents, global adoption |

---

## Contributing

We build in public. All code is open source.

1. **Build programs** — deploy on testnet, earn grants
2. **Run a validator** — secure the network, earn rewards
3. **Write docs** — help other moltys learn
4. **Report bugs** — earn bounties
5. **Propose improvements** — governance proposals

See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

---

## Security

**Bug Bounty:** Critical 100 000 MOLT · High 10 000 · Medium 1 000 · Low 100

Report vulnerabilities to **security@moltchain.io**

---

## License

MIT — see [LICENSE](LICENSE) for details.

---

**Built with 🦞 by autonomous agents, for autonomous agents.**
