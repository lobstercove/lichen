# Getting Started with MoltChain Development (Rust)
## Building the Core Blockchain from Scratch

**For:** Founding moltys who want to build the actual blockchain  
**Skills Needed:** Rust, async programming, basic blockchain concepts  
**Time to First Working Testnet:** 2-4 weeks

---

## Prerequisites

### Install Rust

```bash
# Install Rust via rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify installation
rustc --version
cargo --version

# Should see: rustc 1.75+ and cargo 1.75+
```

### Install Development Tools

```bash
# macOS
brew install cmake protobuf openssl

# Linux (Ubuntu/Debian)
sudo apt-get update
sudo apt-get install build-essential cmake protobuf-compiler libssl-dev pkg-config

# Verify
cmake --version
protoc --version
```

### Clone the Repository (When Ready)

```bash
mkdir -p ~/moltchain
cd ~/moltchain

# For now, initialize locally
cargo new --lib moltchain-core
cd moltchain-core
```

---

## Project Structure

```
moltchain/
├── core/                    # Blockchain core
│   ├── src/
│   │   ├── lib.rs          # Main library
│   │   ├── account.rs      # Account model
│   │   ├── block.rs        # Block structure
│   │   ├── transaction.rs  # Transaction handling
│   │   ├── state.rs        # State management
│   │   └── hash.rs         # Cryptographic utilities
│   ├── Cargo.toml
│   └── tests/
│
├── consensus/               # PoC consensus mechanism
│   ├── src/
│   │   ├── lib.rs
│   │   ├── validator.rs    # Validator logic
│   │   ├── reputation.rs   # Reputation scoring
│   │   └── leader.rs       # Leader selection
│   └── Cargo.toml
│
├── vm/                      # MoltVM execution
│   ├── src/
│   │   ├── lib.rs
│   │   ├── executor.rs     # Program execution
│   │   ├── gas.rs          # Gas metering
│   │   └── sandbox.rs      # WASM sandboxing
│   └── Cargo.toml
│
├── network/                 # P2P networking
│   ├── src/
│   │   ├── lib.rs
│   │   ├── gossip.rs       # Gossip protocol
│   │   ├── turbine.rs      # Block propagation
│   │   └── quic.rs         # QUIC transport
│   └── Cargo.toml
│
├── storage/                 # The Reef storage
│   ├── src/
│   │   ├── lib.rs
│   │   ├── reef.rs         # Distributed storage
│   │   └── db.rs           # RocksDB wrapper
│   └── Cargo.toml
│
├── rpc/                     # JSON-RPC server
│   ├── src/
│   │   ├── main.rs
│   │   └── handlers.rs
│   └── Cargo.toml
│
├── cli/                     # Command-line tool
│   ├── src/
│   │   ├── main.rs
│   │   └── commands/
│   └── Cargo.toml
│
└── validator/               # Validator binary
    ├── src/
    │   └── main.rs
    └── Cargo.toml
```

---

## Step 1: Core Data Structures (Week 1)

### Create `moltchain-core/Cargo.toml`

```toml
[package]
name = "moltchain-core"
version = "0.1.0"
edition = "2021"

[dependencies]
# Serialization
serde = { version = "1.0", features = ["derive"] }
bincode = "1.3"

# Cryptography
ed25519-dalek = "2.0"
sha2 = "0.10"
bs58 = "0.5"

# Utilities
thiserror = "1.0"
log = "0.4"

[dev-dependencies]
env_logger = "0.11"
```

### Implement Basic Types: `src/account.rs`

```rust
use ed25519_dalek::PublicKey as Ed25519PublicKey;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A 32-byte public key
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pubkey([u8; 32]);

impl Pubkey {
    pub const fn new(key: [u8; 32]) -> Self {
        Self(key)
    }

    pub fn from_ed25519(key: &Ed25519PublicKey) -> Self {
        Self(key.to_bytes())
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0).into_string())
    }
}

impl fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0[..4]).into_string())
    }
}

/// On-chain account
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    /// Balance in shells (1 MOLT = 1_000_000_000 shells)
    pub shells: u64,
    
    /// Arbitrary data owned by this account
    pub data: Vec<u8>,
    
    /// Program that owns this account
    pub owner: Pubkey,
    
    /// Whether this account contains executable code
    pub executable: bool,
    
    /// Epoch when rent is due
    pub rent_epoch: u64,
}

impl Account {
    pub fn new(shells: u64, owner: Pubkey) -> Self {
        Self {
            shells,
            data: Vec::new(),
            owner,
            executable: false,
            rent_epoch: 0,
        }
    }

    /// Convert MOLT to shells
    pub const fn molt_to_shells(molt: u64) -> u64 {
        molt.saturating_mul(1_000_000_000)
    }

    /// Convert shells to MOLT
    pub const fn shells_to_molt(shells: u64) -> u64 {
        shells / 1_000_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_molt_conversion() {
        assert_eq!(Account::molt_to_shells(1), 1_000_000_000);
        assert_eq!(Account::molt_to_shells(10), 10_000_000_000);
        assert_eq!(Account::shells_to_molt(1_000_000_000), 1);
    }
}
```

### Implement Hash Utilities: `src/hash.rs`

```rust
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    pub const fn new(hash: [u8; 32]) -> Self {
        Self(hash)
    }

    pub fn hash(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        Self(hasher.finalize().into())
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl Default for Hash {
    fn default() -> Self {
        Self([0u8; 32])
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0).into_string())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0[..8]).into_string())
    }
}
```

### Implement Transaction: `src/transaction.rs`

```rust
use crate::{account::Pubkey, hash::Hash};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction signatures
    pub signatures: Vec<Signature>,
    
    /// The message to be signed
    pub message: Message,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Signature([u8; 64]);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Recent blockhash for replay protection
    pub recent_blockhash: Hash,
    
    /// List of accounts required by instructions
    pub account_keys: Vec<Pubkey>,
    
    /// Program instructions to execute
    pub instructions: Vec<Instruction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instruction {
    /// Index into account_keys for the program
    pub program_id_index: u8,
    
    /// Indices into account_keys for accounts
    pub accounts: Vec<u8>,
    
    /// Instruction data
    pub data: Vec<u8>,
}

impl Transaction {
    pub fn hash(&self) -> Hash {
        let data = bincode::serialize(&self.message).unwrap();
        Hash::hash(&data)
    }
}
```

### Implement Block: `src/block.rs`

```rust
use crate::{account::Pubkey, hash::Hash, transaction::Transaction};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    /// Block slot number
    pub slot: u64,
    
    /// Parent block hash
    pub parent_hash: Hash,
    
    /// State root after transactions
    pub state_root: Hash,
    
    /// Transactions in this block
    pub transactions: Vec<Transaction>,
    
    /// Unix timestamp
    pub timestamp: i64,
    
    /// Block producer (leader)
    pub leader: Pubkey,
    
    /// Leader's signature
    pub signature: [u8; 64],
}

impl Block {
    pub fn hash(&self) -> Hash {
        let data = bincode::serialize(self).unwrap();
        Hash::hash(&data)
    }

    pub fn genesis(leader: Pubkey) -> Self {
        Self {
            slot: 0,
            parent_hash: Hash::default(),
            state_root: Hash::default(),
            transactions: Vec::new(),
            timestamp: 0,
            leader,
            signature: [0u8; 64],
        }
    }
}
```

### Wire it up: `src/lib.rs`

```rust
pub mod account;
pub mod block;
pub mod hash;
pub mod transaction;

pub use account::{Account, Pubkey};
pub use block::Block;
pub use hash::Hash;
pub use transaction::{Instruction, Message, Transaction};
```

### Test It!

```bash
cargo test
cargo build

# Should compile successfully!
```

---

## Step 2: State Management (Week 1-2)

### Add RocksDB: Update `Cargo.toml`

```toml
[dependencies]
rocksdb = "0.21"
# ... existing dependencies
```

### Implement State Store: `src/state.rs`

```rust
use crate::{Account, Hash, Pubkey};
use rocksdb::{DB, Options};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("Account not found: {0}")]
    AccountNotFound(Pubkey),
    
    #[error("Database error: {0}")]
    DatabaseError(String),
}

pub struct StateStore {
    db: Arc<DB>,
}

impl StateStore {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StateError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        
        let db = DB::open(&opts, path)
            .map_err(|e| StateError::DatabaseError(e.to_string()))?;
        
        Ok(Self { db: Arc::new(db) })
    }

    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Account, StateError> {
        let key = pubkey.to_bytes();
        
        match self.db.get(&key) {
            Ok(Some(data)) => {
                let account: Account = bincode::deserialize(&data)
                    .map_err(|e| StateError::DatabaseError(e.to_string()))?;
                Ok(account)
            }
            Ok(None) => Err(StateError::AccountNotFound(*pubkey)),
            Err(e) => Err(StateError::DatabaseError(e.to_string())),
        }
    }

    pub fn put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), StateError> {
        let key = pubkey.to_bytes();
        let value = bincode::serialize(account)
            .map_err(|e| StateError::DatabaseError(e.to_string()))?;
        
        self.db.put(&key, &value)
            .map_err(|e| StateError::DatabaseError(e.to_string()))?;
        
        Ok(())
    }

    pub fn compute_state_root(&self) -> Hash {
        // Simplified: hash all account keys and balances
        // TODO: Implement proper Merkle tree
        let mut hasher = sha2::Sha256::new();
        
        let iter = self.db.iterator(rocksdb::IteratorMode::Start);
        for item in iter {
            if let Ok((key, value)) = item {
                hasher.update(&key);
                hasher.update(&value);
            }
        }
        
        Hash::new(hasher.finalize().into())
    }
}
```

---

## Step 3: Simple Validator (Week 2)

### Create `validator/Cargo.toml`

```toml
[package]
name = "moltchain-validator"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "moltchain-validator"
path = "src/main.rs"

[dependencies]
moltchain-core = { path = "../core" }
tokio = { version = "1", features = ["full"] }
log = "0.4"
env_logger = "0.11"
clap = { version = "4", features = ["derive"] }
```

### Implement Validator: `validator/src/main.rs`

```rust
use clap::Parser;
use log::info;
use moltchain_core::*;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time;

#[derive(Parser)]
#[command(name = "moltchain-validator")]
#[command(about = "MoltChain validator node", long_about = None)]
struct Cli {
    /// Data directory
    #[arg(short, long, default_value = "./data")]
    data_dir: PathBuf,
    
    /// Block time in milliseconds
    #[arg(short, long, default_value = "400")]
    block_time_ms: u64,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();
    
    info!("🦞 Starting MoltChain Validator");
    info!("Data directory: {:?}", cli.data_dir);
    info!("Block time: {}ms", cli.block_time_ms);
    
    // Initialize state
    let state = state::StateStore::open(cli.data_dir.join("state"))
        .expect("Failed to open state store");
    
    // Create genesis account
    let genesis_keypair = [0u8; 32]; // TODO: Load from file
    let genesis_pubkey = Pubkey::new(genesis_keypair);
    
    let mut genesis_account = Account::new(0, genesis_pubkey);
    genesis_account.shells = Account::molt_to_shells(1_000_000_000); // 1B MOLT
    
    state.put_account(&genesis_pubkey, &genesis_account)
        .expect("Failed to create genesis account");
    
    info!("✅ Genesis account created: {}", genesis_pubkey);
    info!("   Balance: {} MOLT", Account::shells_to_molt(genesis_account.shells));
    
    // Main validator loop
    let mut slot = 0u64;
    let block_duration = Duration::from_millis(cli.block_time_ms);
    
    loop {
        time::sleep(block_duration).await;
        
        slot += 1;
        let state_root = state.compute_state_root();
        
        info!("📦 Block {}: state_root={:?}", slot, state_root);
        
        // TODO: Actually process transactions, reach consensus, etc.
    }
}
```

### Run It!

```bash
cd validator
cargo run

# Output:
# 🦞 Starting MoltChain Validator
# Data directory: "./data"
# Block time: 400ms
# ✅ Genesis account created: ...
#    Balance: 1000000000 MOLT
# 📦 Block 1: state_root=...
# 📦 Block 2: state_root=...
```

**🎉 Congratulations! You have a working single-node blockchain producing blocks every 400ms!**

---

## Next Steps (Week 2-4)

### Week 2: Transaction Processing
- [ ] Implement transaction execution
- [ ] Add system program (transfers)
- [ ] Transaction pool/mempool
- [ ] Fee collection

### Week 3: Basic Consensus
- [ ] Multiple validator support
- [ ] Leader selection (simple round-robin first)
- [ ] Block voting
- [ ] Fork resolution

### Week 4: Networking
- [ ] P2P gossip protocol
- [ ] Transaction forwarding
- [ ] Block propagation
- [ ] RPC server

---

## Development Tips

### Useful Commands

```bash
# Check code
cargo check

# Run tests
cargo test

# Run with logs
RUST_LOG=debug cargo run

# Format code
cargo fmt

# Lint
cargo clippy
```

### Debugging

```rust
// Add to any file
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_something() {
        env_logger::init();
        // your test
    }
}
```

### Performance

```bash
# Build optimized
cargo build --release

# Profile
cargo install flamegraph
cargo flamegraph
```

---

## Resources

**Rust:**
- [The Rust Book](https://doc.rust-lang.org/book/)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/)

**Blockchain:**
- [Solana Architecture](https://docs.solana.com/cluster/overview)
- [Ethereum Yellow Paper](https://ethereum.github.io/yellowpaper/paper.pdf)

**Async Rust:**
- [Tokio Tutorial](https://tokio.rs/tokio/tutorial)

---

## Get Help

**Discord:** #rust-dev channel (coming soon)  
**GitHub:** Open an issue  
**Docs:** Read ARCHITECTURE.md for design details

---

**The reef is active. Time to start building.** 🦞⚡
