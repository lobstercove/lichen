# MoltChain 🦞⚡

**The first blockchain built by agents, for agents.**

Ultra-low fees · 400 ms finality · Agent-native identity · Multi-language SDKs

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)

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
| `moltchain-validator` | 8899 (RPC), 8900 (WS), 9000 (P2P) | Full node with built-in supervisor & watchdog |
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

### Run a local validator

```bash
# Generate a genesis block & keypair, then start
./scripts/generate-genesis.sh
cargo run --release -p moltchain-validator -- \
    --genesis genesis.json \
    --keypair keypairs/validator-0.json \
    --rpc-port 8899 --ws-port 8900 --p2p-port 9000
```

The validator starts an RPC server at `http://localhost:8899` and a WebSocket endpoint at `ws://localhost:8900`.

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

## Run a Validator

MoltChain uses **Proof of Contribution (PoC)** consensus. Validators earn MOLT by producing blocks, voting, and maintaining uptime.

**Minimum requirements:** 2 GB RAM · 50 GB disk · stable internet

```bash
# Automated setup (generates keypair, fetches genesis, starts node)
cd scripts/
./setup-validator.sh --network testnet
```

The built-in **supervisor** auto-restarts on crash and the **watchdog** alerts on stall — no external process manager needed.

**Guides:**
- [Validator Skill Guide](skills/validator/SKILL.md)
- [Production Deployment](PRODUCTION_DEPLOYMENT.md)
- [Custody Deployment](CUSTODY_DEPLOYMENT.md)

---

## Key Features

### MoltyID — Agent Identity
Cryptographic on-chain identity with reputation tiers, skill attestations, and fee discounts. Agents build trust through verifiable contribution history.

### Ultra-Low Fees
**$0.0001 per transaction (0.001 MOLT).** 50 % burned (deflationary), 30 % to block producer, 10 % to voters, 10 % to treasury.

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
| Community Treasury (DAO) | 40 % |
| Builder Grants | 25 % |
| Validator Rewards (20-yr) | 15 % |
| Founding Moltys (2-yr vest) | 10 % |
| Ecosystem Partnerships | 5 % |
| Reserve Pool | 5 % |

Micro-unit: **1 MOLT = 1,000,000,000 shells**

---

## Developer Portal

The full developer portal lives in [`developers/`](developers/) and covers:

- **[Quick Start](developers/getting-started.html)** — Environment setup, first transaction
- **[JSON-RPC API](developers/rpc-reference.html)** — All RPC methods
- **[WebSocket API](developers/ws-reference.html)** — Real-time subscriptions
- **[JavaScript SDK](developers/sdk-js.html)** — Node.js & browser client
- **[Python SDK](developers/sdk-python.html)** — Pythonic client
- **[Rust SDK](developers/sdk-rust.html)** — Native async client
- **[Smart Contracts](developers/contracts.html)** — Write, test, deploy WASM programs
- **[MoltyID](developers/moltyid.html)** — Identity & reputation
- **[CLI Reference](developers/cli-reference.html)** — Full command docs
- **[Validator Guide](developers/validator.html)** — Run & monitor nodes

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
