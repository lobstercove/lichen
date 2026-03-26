# Lichen Contract Development Guide

> Complete guide to writing, testing, deploying, and interacting with WASM smart contracts on Lichen.

## Table of Contents

1. [Overview](#overview)
2. [Deploy vs Token Create — When to Use Which](#deploy-vs-token-create)
3. [Project Setup](#project-setup)
4. [Contract SDK API Reference](#sdk-api-reference)
5. [Function Signatures & Calling Convention](#function-signatures)
6. [Writing Your First Contract](#first-contract)
7. [Testing Contracts](#testing)
8. [Building & Deploying](#building-deploying)
9. [Interacting with Deployed Contracts](#interacting)
10. [Security Best Practices](#security)
11. [Appendix: Contract Template](#template)

---

## 1. Overview <a name="overview"></a>

Lichen smart contracts are WASM modules compiled from Rust (`wasm32-unknown-unknown` target). They run in a sandboxed Wasmer VM with access to host-provided functions for storage, logging, events, cross-contract calls, and more.

**Key facts:**
- Language: Rust (compiled to WASM)
- Target: `wasm32-unknown-unknown`
- Environment: `#![no_std]` (no standard library)
- SDK: `lichen-contract-sdk` (path dependency)
- Dispatch: Named WASM exports (`#[no_mangle] pub extern "C" fn`)
- Return convention: `u32` return = 1 for success, 0 for failure. `u64` return for numeric values.
- Deploy fee: 25 LICN + 0.001 LICN base fee
- Max size: 512 KB WASM binary
- Signing: Ed25519

---

## 2. Deploy vs Token Create <a name="deploy-vs-token-create"></a>

Lichen offers two paths to create tokens/contracts:

### `lichen deploy` — Custom WASM Contract
Use when you need **custom logic** beyond a standard token.

```bash
lichen deploy my_contract.wasm --keypair my_key.json
```

- Deploys your compiled WASM code on-chain
- You write and compile the Rust code yourself
- Full access to all SDK features (storage, events, cross-calls, etc.)
- 25 LICN deploy fee
- Address derived from deployer pubkey + code hash

### `licn token create` — Native Fungible Token (No Code)
Use when you just want a **standard MT-20 fungible token** without writing any code.

```bash
licn token create "My Token" MYTOK --supply 1000000 --decimals 9
```

- Creates a standard fungible token via the system program (no WASM involved)
- Fixed functionality: transfer, mint, burn, approve, transfer_from
- No custom logic possible
- Uses instruction type 10 (native system instruction)
- Much cheaper (only base fee)

### Decision Tree

```
Do you need custom logic?
├── YES → lichen deploy (write WASM contract)
│   ├── Custom token with fees/burns/locks? → Write contract using Token module
│   ├── NFT collection? → Write contract using NFT module
│   ├── DeFi protocol? → Write contract with CrossCall + Token
│   ├── Game/DAO/Oracle? → Write custom contract
│   └── Any non-standard behavior → Write custom contract
│
└── NO → licn token create (native token, no code)
    └── Just need a simple fungible token with standard transfer/mint/burn
```

---

## 3. Project Setup <a name="project-setup"></a>

### Prerequisites

```bash
# Install Rust with WASM target
rustup target add wasm32-unknown-unknown

# Install Lichen CLI
cargo install --path cli/       # from Lichen repo
# or download the binary from releases
```

### Create a New Contract

```bash
mkdir my_contract && cd my_contract
cargo init --lib
```

### Cargo.toml

```toml
[workspace]

[package]
name = "my-contract"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
lichen-sdk = { package = "lichen-contract-sdk", path = "../path/to/lichen/sdk" }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

> **Important**: The crate type must be `["cdylib", "rlib"]`. `cdylib` produces the WASM module, `rlib` enables native testing.

---

## 4. Contract SDK API Reference <a name="sdk-api-reference"></a>

The contract SDK (`lichen-contract-sdk`) provides everything contracts need to interact with the Lichen runtime.

### Types

| Type | Description |
|------|-------------|
| `Address` | 32-byte account address. `Address::new(bytes: [u8; 32])` |
| `ContractResult<T>` | `Result<T, ContractError>` |
| `ContractError` | `InsufficientFunds`, `Unauthorized`, `InvalidInput`, `StorageError`, `Overflow`, `Custom(&'static str)` |

### Context Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `get_caller()` | `fn get_caller() -> Address` | Returns the address that invoked this contract |
| `get_contract_address()` | `fn get_contract_address() -> Address` | Returns this contract's own address |
| `get_timestamp()` | `fn get_timestamp() -> u64` | Current block timestamp (seconds since epoch) |
| `get_value()` | `fn get_value() -> u64` | LICN value (spores) sent with this call |
| `get_slot()` | `fn get_slot() -> u64` | Current block slot number |

### Storage

Key-value storage persisted across calls. Keys and values are `&[u8]`.

| Function | Signature | Description |
|----------|-----------|-------------|
| `storage_get(key)` | `fn storage_get(key: &[u8]) -> Option<Vec<u8>>` | Read a value (max 64 KB) |
| `storage_set(key, value)` | `fn storage_set(key: &[u8], value: &[u8]) -> bool` | Write a value |
| `storage::remove(key)` | `fn remove(key: &[u8]) -> bool` | Delete a key (writes empty value) |

### Contract I/O

| Function | Signature | Description |
|----------|-----------|-------------|
| `contract::args()` | `fn args() -> Vec<u8>` | Get the call arguments (raw bytes) |
| `contract::set_return(data)` | `fn set_return(data: &[u8]) -> bool` | Set return data for caller |

### Events & Logging

| Function | Signature | Description |
|----------|-----------|-------------|
| `event::emit(json)` | `fn emit(json: &str) -> bool` | Emit a JSON event |
| `log::info(msg)` | `fn info(msg: &str)` | Log a message to the runtime |

### Utility Functions

| Function | Signature | Description |
|----------|-----------|-------------|
| `u64_to_bytes(n)` | `fn u64_to_bytes(n: u64) -> [u8; 8]` | Convert u64 to little-endian bytes |
| `bytes_to_u64(bytes)` | `fn bytes_to_u64(bytes: &[u8]) -> u64` | Convert bytes to u64 (zero-pads if short) |

### Token Module (MT-20)

Build fungible tokens with the `Token` struct:

```rust
use lichen_sdk::Token;

static mut MY_TOKEN: Token = Token::new("MyToken", "MTK", 9, "mtk");
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `Token::new(name, symbol, decimals, prefix)` | `const fn` | Create token (compile-time) |
| `token.initialize(supply, owner)` | `fn initialize(&mut self, u64, Address)` | Mint initial supply to owner |
| `token.balance_of(account)` | `fn balance_of(&self, Address) -> u64` | Get balance |
| `token.transfer(from, to, amount)` | `fn transfer(&self, Address, Address, u64)` | Transfer tokens |
| `token.mint(to, amount, caller, owner)` | `fn mint(&mut self, Address, u64, Address, Address)` | Mint (owner only) |
| `token.burn(from, amount)` | `fn burn(&mut self, Address, u64)` | Burn tokens |
| `token.approve(owner, spender, amount)` | `fn approve(&self, Address, Address, u64)` | Set allowance |
| `token.transfer_from(caller, from, to, amount)` | `fn transfer_from(&self, Address, Address, Address, u64)` | Transfer using allowance |
| `token.allowance(owner, spender)` | `fn allowance(&self, Address, Address) -> u64` | Get allowance |
| `token.get_total_supply()` | `fn get_total_supply(&self) -> u64` | Get total supply |

**Storage keys** (auto-managed by `prefix`):
- `{prefix}_bal_{hex_address}` — balance
- `{prefix}_alw_{hex_owner}_{hex_spender}` — allowance
- `{prefix}_supply` — total supply

### NFT Module (MT-721)

```rust
use lichen_sdk::NFT;

static mut MY_NFT: NFT = NFT::new("MyNFTs", "MNFT");
```

| Method | Signature |
|--------|-----------|
| `NFT::new(name, symbol)` | Create collection |
| `nft.initialize(minter)` | Set minter address |
| `nft.mint(to, token_id, metadata_uri)` | Mint an NFT |
| `nft.transfer(from, to, token_id)` | Transfer NFT |
| `nft.owner_of(token_id)` | Get owner address |
| `nft.balance_of(owner)` | Count owned NFTs |
| `nft.approve(owner, spender, token_id)` | Approve transfer |
| `nft.burn(owner, token_id)` | Burn an NFT |

### Cross-Contract Calls

```rust
use lichen_sdk::{CrossCall, call_contract, call_token_transfer, call_token_balance};
```

| Function | Signature | Description |
|----------|-----------|-------------|
| `CrossCall::new(target, function, args)` | Builder for cross-calls | |
| `call.with_value(amount)` | Attach LICN to the call | |
| `call_contract(call)` | `fn(CrossCall) -> CallResult<Vec<u8>>` | Execute cross-call |
| `call_token_transfer(token, from, to, amount)` | Shortcut for token transfers | |
| `call_token_balance(token, account)` | Shortcut for balance queries | |
| `call_nft_transfer(nft, from, to, token_id)` | Shortcut for NFT transfers | |
| `call_nft_owner(nft, token_id)` | Shortcut for NFT owner queries | |

### DEX Module (AMM Pool)

```rust
use lichen_sdk::Pool;

static mut MY_POOL: Pool = Pool::new(
    Address::new([0u8; 32]),  // token_a
    Address::new([0u8; 32]),  // token_b
);
```

| Method | Signature |
|--------|-----------|
| `pool.initialize(token_a, token_b)` | Initialize pool |
| `pool.add_liquidity(provider, amount_a, amount_b, min_liq)` | Add liquidity |
| `pool.remove_liquidity(provider, liquidity, min_a, min_b)` | Remove liquidity |
| `pool.swap_a_for_b(amount_in, min_out)` | Swap A→B |
| `pool.swap_b_for_a(amount_in, min_out)` | Swap B→A |
| `pool.get_amount_out(amount_in, reserve_in, reserve_out)` | Price quote |
| `pool.save()` | Persist pool state |
| `pool.load()` | Load pool state |

---

## 5. Function Signatures & Calling Convention <a name="function-signatures"></a>

### Export Convention

Every public function must be:
```rust
#[no_mangle]
pub extern "C" fn function_name(/* params */) -> u32 {
    // ...
    1 // success
}
```

### Parameter Types

Lichen's WASM VM passes parameters as follows:

| Rust Type | WASM Type | Usage |
|-----------|-----------|-------|
| `*const u8` | `i32` (pointer) | 32-byte addresses — read with `unsafe { core::slice::from_raw_parts(ptr, 32) }` |
| `u64` | `i64` | Amounts, token IDs, timestamps |
| `u32` | `i32` | Flags, small integers |

### Return Convention

| Return Type | Convention |
|-------------|-----------|
| `u32` | `1` = success, `0` = failure |
| `u64` | Numeric value (balances, counts, etc.) |

### Reading Pointer Parameters

```rust
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from = unsafe {
        let slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        Address::new(arr)
    };
    let to = unsafe {
        let slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        Address::new(arr)
    };
    // ... implementation
    1
}
```

### Alternative: Using `contract::args()`

For functions that take complex or variable-length arguments:

```rust
#[no_mangle]
pub extern "C" fn complex_action() -> u32 {
    let args = lichen_sdk::contract::args();
    // Parse args as JSON, bincode, or custom format
    // ...
    1
}
```

### Setting Return Data

```rust
#[no_mangle]
pub extern "C" fn get_stats() -> u32 {
    let data = b"some result bytes";
    lichen_sdk::contract::set_return(data);
    1
}
```

---

## 6. Writing Your First Contract <a name="first-contract"></a>

### Example: Counter Contract

A minimal contract that stores and increments a counter.

```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use lichen_sdk::{
    storage_get, storage_set, log_info, get_caller, set_return_data,
    u64_to_bytes, bytes_to_u64, Address,
};

const COUNTER_KEY: &[u8] = b"counter";
const OWNER_KEY: &[u8] = b"owner";

/// Called once after deployment to set the owner.
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    let owner = unsafe {
        let slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        arr
    };
    storage_set(OWNER_KEY, &owner);
    storage_set(COUNTER_KEY, &u64_to_bytes(0));
    log_info("Counter initialized");
    1
}

/// Increment the counter by 1. Anyone can call this.
#[no_mangle]
pub extern "C" fn increment() -> u32 {
    let current = storage_get(COUNTER_KEY)
        .map(|v| bytes_to_u64(&v))
        .unwrap_or(0);
    let new_value = current + 1;
    storage_set(COUNTER_KEY, &u64_to_bytes(new_value));
    set_return_data(&u64_to_bytes(new_value));
    1
}

/// Get the current counter value.
#[no_mangle]
pub extern "C" fn get_count() -> u64 {
    storage_get(COUNTER_KEY)
        .map(|v| bytes_to_u64(&v))
        .unwrap_or(0)
}

/// Reset counter to 0 (owner only).
#[no_mangle]
pub extern "C" fn reset() -> u32 {
    let caller = get_caller();
    let owner = match storage_get(OWNER_KEY) {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Address::new(arr)
        }
        _ => return 0,
    };
    if caller.0 != owner.0 {
        log_info("Unauthorized: only owner can reset");
        return 0;
    }
    storage_set(COUNTER_KEY, &u64_to_bytes(0));
    1
}
```

### Example: Token Contract (Using SDK Token Module)

```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use lichen_sdk::{
    Token, Address, log_info, get_caller, set_return_data,
    u64_to_bytes, bytes_to_u64, storage_get, storage_set,
};

static mut TOKEN: Token = Token::new("GameGold", "GGLD", 9, "ggld");

const ADMIN_KEY: &[u8] = b"admin";

fn get_admin() -> Address {
    match storage_get(ADMIN_KEY) {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Address::new(arr)
        }
        _ => Address::new([0u8; 32]),
    }
}

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    let owner = unsafe {
        let slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        Address::new(arr)
    };
    storage_set(ADMIN_KEY, &owner.0);
    let initial_supply = 1_000_000_000_000_000; // 1M tokens with 9 decimals
    unsafe {
        if TOKEN.initialize(initial_supply, owner).is_ok() { 1 } else { 0 }
    }
}

#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from = unsafe {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(core::slice::from_raw_parts(from_ptr, 32));
        Address::new(arr)
    };
    let to = unsafe {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(core::slice::from_raw_parts(to_ptr, 32));
        Address::new(arr)
    };
    unsafe {
        if TOKEN.transfer(from, to, amount).is_ok() { 1 } else { 0 }
    }
}

#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let account = unsafe {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(core::slice::from_raw_parts(account_ptr, 32));
        Address::new(arr)
    };
    unsafe { TOKEN.balance_of(account) }
}

#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    unsafe { TOKEN.get_total_supply() }
}
```

---

## 7. Testing Contracts <a name="testing"></a>

The SDK includes a `test_mock` module that provides thread-local mock implementations of all host functions. Tests run natively (not in WASM) using `cargo test`.

### Setup

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use lichen_sdk::test_mock;

    #[test]
    fn test_initialize_and_increment() {
        test_mock::reset();

        let owner = [1u8; 32];
        test_mock::set_caller(owner);

        // Initialize
        assert_eq!(initialize(owner.as_ptr()), 1);

        // Increment
        assert_eq!(increment(), 1);
        assert_eq!(get_count(), 1);

        // Increment again
        assert_eq!(increment(), 1);
        assert_eq!(get_count(), 2);
    }

    #[test]
    fn test_reset_owner_only() {
        test_mock::reset();

        let owner = [1u8; 32];
        let other = [2u8; 32];

        test_mock::set_caller(owner);
        initialize(owner.as_ptr());
        increment();
        assert_eq!(get_count(), 1);

        // Non-owner cannot reset
        test_mock::set_caller(other);
        assert_eq!(reset(), 0);
        assert_eq!(get_count(), 1);

        // Owner can reset
        test_mock::set_caller(owner);
        assert_eq!(reset(), 1);
        assert_eq!(get_count(), 0);
    }
}
```

### Test Mock Functions

| Function | Description |
|----------|-------------|
| `test_mock::reset()` | Clear all mock state |
| `test_mock::set_caller(addr)` | Set the caller address |
| `test_mock::set_contract_address(addr)` | Set contract's own address |
| `test_mock::set_args(data)` | Set call arguments |
| `test_mock::set_timestamp(ts)` | Set block timestamp |
| `test_mock::set_value(val)` | Set LICN value sent |
| `test_mock::set_slot(s)` | Set current slot |
| `test_mock::get_return_data()` | Read return data set by contract |
| `test_mock::get_events()` | Read emitted events |
| `test_mock::get_storage(key)` | Read storage directly |
| `test_mock::get_logs()` | Read logged messages |
| `test_mock::set_cross_call_response(data)` | Mock cross-contract call responses |

### Running Tests

```bash
# Run native tests (not WASM)
cargo test

# Run with output
cargo test -- --nocapture
```

---

## 8. Building & Deploying <a name="building-deploying"></a>

### Build

```bash
cargo build --target wasm32-unknown-unknown --release
```

The WASM file is at:
```
target/wasm32-unknown-unknown/release/my_contract.wasm
```

### Optimize (Optional)

```bash
# Install wasm-opt
# brew install binaryen  (macOS)
# apt install binaryen   (Ubuntu)

wasm-opt -Oz -o optimized.wasm target/wasm32-unknown-unknown/release/my_contract.wasm
```

### Deploy

```bash
# Ensure you have enough LICN (25.001 LICN minimum)
lichen balance

# Deploy the contract
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm --keypair my_key.json
```

Output:
```
🦞 Deploying contract: my_contract.wasm
📦 Size: 42 KB
📍 Contract address: 7Xk9...abc
👤 Deployer: 5Yz2...def
💰 Deploy fee: 25.001 LICN (25 LICN deploy + 0.001 LICN base fee)

✅ Contract deployed!
📝 Signature: 3abc...789
🔗 Address: 7Xk9...abc
```

### Deploy Fee

| Component | Amount |
|-----------|--------|
| Base fee | 0.001 LICN |
| Deploy premium | 25 LICN |
| **Total** | **25.001 LICN** |

If the deployment fails (e.g., invalid WASM, contract already exists), the 25 LICN deploy premium is **refunded**. Only the 0.001 LICN base fee is kept.

### Pre-flight Checks (CLI)

The `lichen deploy` CLI validates before sending:
- WASM magic bytes (`\0asm`)
- File size ≤ 512 KB
- File is not empty

---

## 9. Interacting with Deployed Contracts <a name="interacting"></a>

### CLI

```bash
# Call a function
lichen call <contract_address> <function_name> [args...]

# Example: increment counter
lichen call 7Xk9...abc increment

# Example: transfer tokens (addresses are base58)
lichen call 7Xk9...abc transfer '["5Yz2...", "8Ab3...", 1000000000]'

# Example: read-only query
lichen call 7Xk9...abc get_count
```

### JavaScript SDK

```javascript
import { LichenClient } from '@lichen/sdk';

const client = new LichenClient('https://rpc.lichen.network');

// Call a contract function
const result = await client.callContract(
    contractAddress,
    'increment',
    [],       // args
    keypair   // signer
);

// Read contract state
const count = await client.callContract(
    contractAddress,
    'get_count',
    []
);
```

### RPC (Direct)

```bash
# Deploy via RPC
curl -X POST https://rpc.lichen.network -H 'Content-Type: application/json' -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "sendTransaction",
    "params": ["<base64_encoded_transaction>"]
}'

# Read contract state (with optional caller context)
curl -X POST https://rpc.lichen.network -H 'Content-Type: application/json' -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "callContract",
    "params": {
        "contract": "<contract_address>",
        "function": "get_count",
        "args": [],
        "from": "<caller_pubkey_optional>"
    }
}'
```

---

## 10. Security Best Practices <a name="security"></a>

### Reentrancy Guard

```rust
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
        return false; // Already entered
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}

#[no_mangle]
pub extern "C" fn withdraw(to_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() { return 0; }
    // ... transfer logic ...
    reentrancy_exit();
    1
}
```

### Admin/Owner Checks

```rust
fn require_owner() -> bool {
    let caller = get_caller();
    let owner = match storage_get(b"owner") {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => return false,
    };
    caller.0 == owner
}
```

### Integer Overflow Protection

```rust
// Use saturating arithmetic
let new_balance = balance.checked_add(amount).unwrap_or(u64::MAX);
// Or with explicit check
let new_balance = match balance.checked_add(amount) {
    Some(v) => v,
    None => { log_info("Overflow"); return 0; }
};
```

### Input Validation

```rust
// Validate pointer-based inputs
if from_ptr.is_null() || to_ptr.is_null() {
    return 0;
}
if amount == 0 {
    return 0;
}
```

---

## 11. Appendix: Contract Template <a name="template"></a>

### Full Starter Template

Copy this as your starting point for a new contract:

**Cargo.toml:**
```toml
[workspace]

[package]
name = "my-contract"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
lichen-sdk = { package = "lichen-contract-sdk", path = "../../sdk" }

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

**src/lib.rs:**
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;
use lichen_sdk::{
    storage_get, storage_set, log_info, get_caller, set_return_data,
    u64_to_bytes, bytes_to_u64, Address, emit_event,
};

// ─── Constants ──────────────────────────────────────────────
const OWNER_KEY: &[u8] = b"owner";
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

// ─── Helpers ────────────────────────────────────────────────

fn read_address(ptr: *const u8) -> Address {
    unsafe {
        let slice = core::slice::from_raw_parts(ptr, 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(slice);
        Address::new(arr)
    }
}

fn get_owner() -> Address {
    match storage_get(OWNER_KEY) {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Address::new(arr)
        }
        _ => Address::new([0u8; 32]),
    }
}

fn require_owner() -> bool {
    get_caller().0 == get_owner().0
}

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
        return false;
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}

// ─── Exports ────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    let owner = read_address(owner_ptr);
    storage_set(OWNER_KEY, &owner.0);
    log_info("Contract initialized");
    emit_event(r#"{"type":"initialized"}"#);
    1
}

// Add your contract functions here...

// ─── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_sdk::test_mock;

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let owner = [1u8; 32];
        assert_eq!(initialize(owner.as_ptr()), 1);
        assert_eq!(test_mock::get_storage(OWNER_KEY), Some(owner.to_vec()));
    }
}
```

### Build & Deploy Cheat Sheet

```bash
# Build
cargo build --target wasm32-unknown-unknown --release

# Test
cargo test

# Deploy
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm

# Call a function
lichen call <address> <function> [args]
```
