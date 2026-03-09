# MoltChain Project Structure
## Complete Code Organization

**Last Updated:** February 5, 2026

---

## Overview

```
moltchain/
├── README.md                    # Project overview
├── ROADMAP.md                   # Development timeline
├── STATUS.md                    # Current progress
├── QUICK_REFERENCE.md           # 5-minute overview
├── GETTING_STARTED_RUST.md      # Rust development guide
├── EASY_NODE_OPERATION.md       # Agent-friendly node setup
├── LICENSE                      # Apache 2.0
│
├── docs/                        # Complete documentation
│   ├── WHITEPAPER.md           # Technical & economic vision
│   ├── ARCHITECTURE.md         # Deep technical spec
│   ├── GETTING_STARTED.md      # Developer onboarding
│   ├── VISION.md               # Project manifesto
│   ├── API_REFERENCE.md        # (Coming soon)
│   ├── VALIDATOR_GUIDE.md      # (Coming soon)
│   └── PROGRAM_GUIDE.md        # (Coming soon)
│
├── core/                        # Blockchain core (Rust)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs              # Main library
│   │   ├── account.rs          # Account model (Pubkey, Account)
│   │   ├── block.rs            # Block structure
│   │   ├── transaction.rs      # Transaction handling
│   │   ├── hash.rs             # SHA-256 utilities
│   │   ├── state.rs            # State management (RocksDB)
│   │   └── signature.rs        # Ed25519 signatures
│   └── tests/
│       └── integration.rs
│
├── consensus/                   # Proof of Contribution
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── validator.rs        # Validator logic
│   │   ├── reputation.rs       # Reputation scoring
│   │   ├── leader.rs           # Leader selection
│   │   ├── voting.rs           # BFT voting
│   │   └── slashing.rs         # Slashing conditions
│   └── tests/
│
├── vm/                          # MoltVM (Execution environment)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── executor.rs         # WASM execution
│   │   ├── gas.rs              # Gas metering
│   │   ├── sandbox.rs          # Security sandbox
│   │   ├── rust_runtime.rs     # Rust program support
│   │   ├── js_runtime.rs       # JavaScript runtime
│   │   └── python_runtime.rs   # Python runtime
│   └── tests/
│
├── network/                     # P2P networking
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── gossip.rs           # Gossip protocol
│   │   ├── turbine.rs          # Block propagation
│   │   ├── quic.rs             # QUIC transport
│   │   ├── discovery.rs        # Peer discovery
│   │   └── rpc.rs              # RPC protocol
│   └── tests/
│
├── storage/                     # The Reef (Distributed storage)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs
│   │   ├── reef.rs             # Storage protocol
│   │   ├── db.rs               # RocksDB wrapper
│   │   ├── snapshots.rs        # Snapshot system
│   │   ├── content.rs          # Content addressing
│   │   └── incentives.rs       # Storage rewards
│   └── tests/
│
├── rpc/                         # JSON-RPC server
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── server.rs           # HTTP/WebSocket server
│   │   ├── handlers.rs         # RPC method handlers
│   │   └── subscriptions.rs    # WebSocket subscriptions
│   └── tests/
│
├── validator/                   # Validator binary
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── config.rs           # Configuration
│   │   ├── monitor.rs          # Health monitoring
│   │   └── metrics.rs          # Prometheus metrics
│   └── docker/
│       ├── Dockerfile
│       └── docker-compose.yml
│
├── cli/                         # molt CLI tool
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   └── commands/
│   │       ├── mod.rs
│   │       ├── node.rs         # molt node start/stop/status
│   │       ├── wallet.rs       # molt wallet create/balance
│   │       ├── transfer.rs     # molt transfer
│   │       ├── program.rs      # molt program deploy/call
│   │       ├── validator.rs    # molt validator setup
│   │       ├── config.rs       # molt config set/get
│   │       └── pool.rs         # molt pool create/join
│   └── tests/
│
├── wallet/                      # MoltWallet
│   ├── desktop/                # Electron app
│   │   ├── package.json
│   │   ├── src/
│   │   └── build/
│   ├── mobile/                 # React Native
│   │   ├── ios/
│   │   ├── android/
│   │   └── src/
│   ├── extension/              # Browser extension
│   │   ├── manifest.json
│   │   └── src/
│   └── cli/                    # CLI wallet (part of molt CLI)
│
├── explorer/                    # Reef Explorer
│   ├── frontend/               # Next.js
│   │   ├── package.json
│   │   ├── pages/
│   │   ├── components/
│   │   └── public/
│   ├── backend/                # Indexer
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── indexer.rs     # Block indexing
│   │   │   ├── api.rs         # REST API
│   │   │   └── search.rs      # Search functionality
│   │   └── migrations/        # Database migrations
│   └── docker-compose.yml
│
├── sdk/                         # Software Development Kits
│   ├── rust/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── client.rs      # RPC client
│   │   │   ├── program.rs     # Program interface
│   │   │   ├── transaction.rs # Transaction builder
│   │   │   └── wallet.rs      # Wallet utilities
│   │   └── examples/
│   │
│   ├── js/                     # JavaScript/TypeScript
│   │   ├── package.json
│   │   ├── src/
│   │   │   ├── index.ts
│   │   │   ├── connection.ts  # RPC connection
│   │   │   ├── program.ts     # Program interface
│   │   │   ├── transaction.ts # Transaction builder
│   │   │   └── wallet.ts      # Wallet utilities
│   │   └── examples/
│   │
│   └── python/                 # Python
│       ├── setup.py
│       ├── moltchain/
│       │   ├── __init__.py
│       │   ├── client.py      # RPC client
│       │   ├── program.py     # Program interface
│       │   ├── transaction.py # Transaction builder
│       │   └── wallet.py      # Wallet utilities
│       └── examples/
│
├── programs/                    # Core on-chain programs
│   ├── system/                 # System program (transfers)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── transfer.rs
│   │       └── allocate.rs
│   │
│   ├── token/                  # MTS Token Standard
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── mint.rs
│   │       ├── transfer.rs
│   │       ├── burn.rs
│   │       └── metadata.rs
│   │
│   ├── moltyid/                # Agent Identity System
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── register.rs
│   │       ├── reputation.rs
│   │       └── skills.rs
│   │
│   ├── clawswap/               # DEX
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── pool.rs
│   │       ├── swap.rs
│   │       ├── liquidity.rs
│   │       └── amm.rs
│   │
│   ├── clawpump/               # Token Launchpad
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── launch.rs
│   │       ├── bonding_curve.rs
│   │       └── vesting.rs
│   │
│   ├── lobsterlend/            # Lending Protocol
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── deposit.rs
│   │       ├── borrow.rs
│   │       ├── liquidate.rs
│   │       └── interest.rs
│   │
│   └── reefstake/              # Liquid Staking
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── stake.rs
│           ├── unstake.rs
│           └── rewards.rs
│
├── tests/                       # Integration tests
│   ├── e2e/                    # End-to-end tests
│   ├── performance/            # Performance benchmarks
│   └── security/               # Security tests
│
├── scripts/                     # Utility scripts
│   ├── install.sh              # One-command installer
│   ├── setup-dev.sh            # Dev environment setup
│   ├── build-all.sh            # Build entire project
│   ├── test-all.sh             # Run all tests
│   └── deploy-testnet.sh       # Deploy testnet
│
├── docker/                      # Docker configurations
│   ├── validator/
│   │   └── Dockerfile
│   ├── rpc/
│   │   └── Dockerfile
│   ├── explorer/
│   │   └── Dockerfile
│   └── docker-compose.yml      # Full stack
│
├── .github/                     # GitHub configuration
│   ├── workflows/
│   │   ├── ci.yml              # Continuous integration
│   │   ├── release.yml         # Release automation
│   │   └── security.yml        # Security scanning
│   └── ISSUE_TEMPLATE/
│
└── benches/                     # Rust benchmarks
    ├── consensus.rs
    ├── vm.rs
    └── network.rs
```

---

## Key Directories Explained

### `/core`
The heart of MoltChain. Implements account model, transactions, blocks, and state management. This is where the blockchain fundamentals live.

### `/consensus`
Proof of Contribution consensus mechanism. Handles validator selection, leader scheduling, BFT voting, and slashing.

### `/vm`
MoltVM execution environment. Runs programs written in Rust, JavaScript, or Python. Includes gas metering and security sandboxing.

### `/network`
P2P networking layer using QUIC protocol. Implements gossip, block propagation (Turbine), and peer discovery.

### `/storage`
The Reef distributed storage system. IPFS-like content-addressed storage with economic incentives for validators.

### `/programs`
Core on-chain programs that ship with MoltChain:
- **system:** Basic token transfers
- **token:** MTS token standard (like SPL tokens)
- **moltyid:** Agent identity and reputation
- **clawswap:** Decentralized exchange
- **clawpump:** Token launchpad
- **lobsterlend:** Lending protocol
- **reefstake:** Liquid staking

### `/sdk`
Software development kits in Rust, JavaScript, and Python. Agents use these to interact with the chain.

### `/cli`
The `molt` command-line tool. One interface for everything: nodes, wallets, programs, validators.

### `/wallet`
MoltWallet in multiple forms:
- Desktop (Electron)
- Mobile (iOS/Android via React Native)
- Browser extension
- CLI (integrated with molt CLI)

### `/explorer`
Reef Explorer block explorer:
- Frontend: Next.js web app
- Backend: Rust indexer + REST API
- Real-time transaction viewing
- Program source code display
- Network statistics

---

## Development Workflow

### **Initial Setup**

```bash
# Clone repo (when public)
git clone https://github.com/moltchain/moltchain
cd moltchain

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Node.js (for SDK/wallet/explorer)
# Using nvm:
nvm install 20
nvm use 20

# Install Python (for SDK)
# Python 3.9+ required

# Setup development environment
./scripts/setup-dev.sh
```

### **Build Everything**

```bash
# Build all Rust components
./scripts/build-all.sh

# Or individually:
cd core && cargo build --release
cd consensus && cargo build --release
cd vm && cargo build --release
# etc.

# Build SDKs
cd sdk/js && npm install && npm run build
cd sdk/python && pip install -e .

# Build wallet
cd wallet/desktop && npm install && npm run build

# Build explorer
cd explorer/frontend && npm install && npm run build
cd explorer/backend && cargo build --release
```

### **Run Tests**

```bash
# All tests
./scripts/test-all.sh

# Individual components
cd core && cargo test
cd consensus && cargo test

# Integration tests
cd tests/e2e && cargo test

# Performance benchmarks
cargo bench
```

### **Run Local Testnet**

```bash
# Start single-node testnet
cargo run --bin moltchain-validator -- \
  --data-dir ./test-data \
  --network localnet

# Start full stack (validator + RPC + explorer)
docker-compose up
```

---

## Code Standards

### **Rust**
- Style: `cargo fmt`
- Linting: `cargo clippy`
- Documentation: All public APIs must have doc comments
- Testing: Unit tests in same file, integration tests in /tests

### **JavaScript/TypeScript**
- Style: Prettier
- Linting: ESLint
- Testing: Jest
- Type coverage: 100% for TypeScript

### **Python**
- Style: Black
- Linting: Flake8
- Type hints: Required for all functions
- Testing: Pytest

---

## Documentation

### **For Developers**
- [GETTING_STARTED.md](./docs/GETTING_STARTED.md) - Start here
- [GETTING_STARTED_RUST.md](./GETTING_STARTED_RUST.md) - Rust deep dive
- [ARCHITECTURE.md](./docs/ARCHITECTURE.md) - System design
- API_REFERENCE.md (coming soon) - Complete API docs

### **For Validators**
- [EASY_NODE_OPERATION.md](./EASY_NODE_OPERATION.md) - Node setup
- VALIDATOR_GUIDE.md (coming soon) - Advanced validator config

### **For Agent Builders**
- PROGRAM_GUIDE.md (coming soon) - Writing programs
- SDK docs in each SDK directory
- Examples in `/sdk/*/examples/`

---

## Contributing

1. Fork the repository
2. Create your feature branch
3. Write tests
4. Ensure all tests pass
5. Submit pull request

See CONTRIBUTING.md (coming soon) for details.

---

## License

**MoltChain Core:** Apache 2.0  
**SDKs / CLI / auxiliary packages:** MIT  
**Documentation:** CC BY 4.0

Current licenses are permissive and do not block third-party forks or blockchain deployments. Restricting that would require a license change for the affected code, not a docs-only update.

---

**Everything organized. Everything documented. Ready to build.** 🦞⚡
