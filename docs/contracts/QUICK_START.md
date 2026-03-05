# 🦞 MoltChain Contract Development Guide

## Quick Start

### 1. Install Rust & WASM Target

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASM target
rustup target add wasm32-unknown-unknown
```

### 2. Create Your First Contract

```bash
cd moltchain/contracts
cargo new my_token --lib
cd my_token
```

**Cargo.toml:**
```toml
[package]
name = "my_token"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
moltchain-sdk = { path = "../../sdk" }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
```

**src/lib.rs:**
```rust
#![no_std]
#![no_main]

use moltchain_sdk::{Token, Address, log_info};

static mut TOKEN: Option<Token> = None;

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) {
    // Initialize your token
    unsafe {
        TOKEN = Some(Token::new("MyToken", "MTK", 9, "mtk"));
    }
    log_info("Token initialized!");
}

#[no_mangle]
pub extern "C" fn transfer(from: *const u8, to: *const u8, amount: u64) -> u32 {
    // Implement transfer logic
    1 // Success
}
```

### 3. Build Contract

```bash
cargo build --target wasm32-unknown-unknown --release
```

Your WASM is at: `target/wasm32-unknown-unknown/release/my_token.wasm`

### 4. Deploy Contract

```python
python3 tools/deploy_contract.py my_token.wasm
```

## MT-20 Token Standard

### Interface

```rust
pub trait MT20 {
    fn initialize(owner: Address, initial_supply: u64);
    fn balance_of(account: Address) -> u64;
    fn transfer(from: Address, to: Address, amount: u64) -> bool;
    fn approve(owner: Address, spender: Address, amount: u64) -> bool;
    fn transfer_from(caller: Address, from: Address, to: Address, amount: u64) -> bool;
    fn mint(to: Address, amount: u64, caller: Address, owner: Address) -> bool;
    fn burn(from: Address, amount: u64) -> bool;
    fn total_supply() -> u64;
}
```

### Example Token

```rust
use moltchain_sdk::Token;

let mut token = Token::new("MyCoin", "MYC", 9, "myc");

// Initialize with 1 million tokens
token.initialize(1_000_000_000_000_000, owner_address)?;

// Transfer tokens
token.transfer(sender, receiver, 100_000_000_000)?;

// Approve spending
token.approve(owner, spender, 50_000_000_000)?;

// Spend allowance
token.transfer_from(spender, owner, recipient, 25_000_000_000)?;
```

## Storage API

### Reading Storage

```rust
use moltchain_sdk::storage_get;

let value = storage_get(b"my_key");
if let Some(bytes) = value {
    // Use bytes
}
```

### Writing Storage

```rust
use moltchain_sdk::storage_set;

storage_set(b"my_key", b"my_value");
```

### Removing from Storage

```rust
use moltchain_sdk::storage;

storage::remove(b"my_key");
```

## Gas Management

```rust
use moltchain_sdk::consume_gas;

// Consume 1000 gas units
if consume_gas(1000) {
    // Continue execution
} else {
    // Out of gas!
}
```

## Logging

```rust
use moltchain_sdk::log_info;

log_info("Transfer successful");
log_info("Balance updated");
```

## Advanced Examples

### Counter Contract

```rust
#![no_std]
#![no_main]

use moltchain_sdk::{storage_get, storage_set, bytes_to_u64, u64_to_bytes};

#[no_mangle]
pub extern "C" fn increment() -> u64 {
    let count = storage_get(b"count")
        .map(|b| bytes_to_u64(&b))
        .unwrap_or(0);
    
    let new_count = count + 1;
    storage_set(b"count", &u64_to_bytes(new_count));
    
    new_count
}

#[no_mangle]
pub extern "C" fn get_count() -> u64 {
    storage_get(b"count")
        .map(|b| bytes_to_u64(&b))
        .unwrap_or(0)
}
```

### Multi-Sig Wallet

```rust
#![no_std]
#![no_main]

use moltchain_sdk::{Address, storage_get, storage_set};

static mut OWNERS: [Address; 3] = [Address([0u8; 32]); 3];
static mut REQUIRED: u8 = 2; // 2 of 3 signatures

#[no_mangle]
pub extern "C" fn initialize(owner1: *const u8, owner2: *const u8, owner3: *const u8) {
    // Initialize owners
}

#[no_mangle]
pub extern "C" fn propose_transaction(to: Address, amount: u64) -> u64 {
    // Create proposal
    // Return proposal ID
    0
}

#[no_mangle]
pub extern "C" fn approve_transaction(proposal_id: u64, signer: Address) -> u32 {
    // Check if signer is owner
    // Add signature
    // If threshold reached, execute
    1
}
```

### NFT Contract

```rust
#![no_std]
#![no_main]

use moltchain_sdk::{Address, storage_get, storage_set};

#[no_mangle]
pub extern "C" fn mint(to: Address, token_id: u64, metadata: *const u8, len: u32) -> u32 {
    // Mint NFT
    // Set owner
    // Store metadata
    1
}

#[no_mangle]
pub extern "C" fn transfer(from: Address, to: Address, token_id: u64) -> u32 {
    // Verify ownership
    // Transfer NFT
    1
}

#[no_mangle]
pub extern "C" fn owner_of(token_id: u64) -> Address {
    // Return owner address
    Address([0u8; 32])
}
```

## Gas Costs

| Operation | Gas Cost |
|-----------|----------|
| Base execution | 100 |
| Storage read | 200 |
| Storage write | 5,000 |
| Function call | 700 |
| Transfer | 500 |

## Best Practices

### 1. Minimize Storage Writes
Storage writes are expensive! Cache values in memory when possible.

```rust
// ❌ Bad - multiple writes
for i in 0..10 {
    storage_set(b"count", &u64_to_bytes(i));
}

// ✅ Good - single write
let final_count = 10;
storage_set(b"count", &u64_to_bytes(final_count));
```

### 2. Use Efficient Data Structures
Pack data tightly to reduce storage costs.

```rust
// Pack multiple values into single storage slot
let packed = (value1 as u128) << 64 | (value2 as u128);
```

### 3. Check Inputs Early
Fail fast to save gas.

```rust
#[no_mangle]
pub extern "C" fn transfer(amount: u64) -> u32 {
    if amount == 0 {
        return 0; // Fail immediately
    }
    
    // Continue with expensive operations
}
```

### 4. Emit Logs for Events
Use logs to notify off-chain systems.

```rust
log_info("Transfer: 100 tokens from Alice to Bob");
```

## Deployment

### Via Python Tool

```bash
python3 tools/deploy_contract.py my_token.wasm
```

### Via RPC

```bash
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "deployContract",
    "params": {
      "bytecode": "0x...",
      "deployer": "2kRPL..."
    }
  }'
```

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer() {
        let from = Address([1u8; 32]);
        let to = Address([2u8; 32]);
        
        // Test logic
    }
}
```

### Integration Tests

Build and deploy to local network:

```bash
# Start validator
moltchain-validator 8000

# Deploy contract
python3 tools/deploy_contract.py my_token.wasm

# Test interactions
python3 tools/test_contract.py
```

## Optimization Tips

### Binary Size
- Use `opt-level = "z"` in Cargo.toml
- Enable LTO: `lto = true`
- Strip symbols: `strip = true`
- Use `wasm-opt` for further optimization:

```bash
wasm-opt -Oz input.wasm -o output.wasm
```

### Gas Usage
- Cache storage reads
- Batch storage writes
- Use early returns
- Minimize loop iterations

## Common Patterns

### Access Control

```rust
static mut OWNER: Address = Address([0u8; 32]);

fn only_owner(caller: Address) -> bool {
    unsafe { caller == OWNER }
}

#[no_mangle]
pub extern "C" fn admin_function(caller: *const u8) -> u32 {
    let caller = /* parse address */;
    
    if !only_owner(caller) {
        return 0; // Unauthorized
    }
    
    // Admin logic
    1
}
```

### Reentrancy Guard

```rust
static mut LOCKED: bool = false;

#[no_mangle]
pub extern "C" fn protected_function() -> u32 {
    unsafe {
        if LOCKED {
            return 0; // Already executing
        }
        LOCKED = true;
    }
    
    // Critical section
    
    unsafe { LOCKED = false; }
    1
}
```

### Pausable

```rust
static mut PAUSED: bool = false;

fn when_not_paused() -> bool {
    unsafe { !PAUSED }
}

#[no_mangle]
pub extern "C" fn pause(caller: *const u8) -> u32 {
    if !only_owner(caller) {
        return 0;
    }
    unsafe { PAUSED = true; }
    1
}
```

## Resources

- **SDK Docs**: `moltchain/sdk/README.md`
- **Examples**: `moltchain/contracts/`
- **RPC API**: `http://localhost:8899/docs`
- **Discord**: Join for support

---

**Happy Contracting! 🦞💻**
