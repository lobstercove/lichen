# MoltChain Rust SDK

Official Rust SDK for building on MoltChain blockchain.

## Features

- ✅ **Production-Ready** - Type-safe, async RPC client
- ✅ **Ed25519 Keypairs** - Industry-standard cryptography
- ✅ **Solana-Compatible** - Compatible wallet formats
- ✅ **Transaction Building** - Easy transaction creation and signing
- ✅ **Developer-Friendly** - Comprehensive examples and docs

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
moltchain-sdk = { path = "../path/to/sdk/rust" }
tokio = { version = "1.35", features = ["full"] }
```

## Quick Start

```rust
use moltchain_sdk::{Client, Keypair};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to validator
    let client = Client::new("http://localhost:8899");
    
    // Generate keypair
    let keypair = Keypair::new();
    println!("Public key: {}", keypair.pubkey().to_base58());
    
    // Get balance
    let balance = client.get_balance(&keypair.pubkey()).await?;
    println!("Balance: {} MOLT", balance.molt());
    
    // Get current slot
    let slot = client.get_slot().await?;
    println!("Current slot: {}", slot);
    
    Ok(())
}
```

## Examples

Run examples with:

```bash
cargo run --example basic
cargo run --example transfer
cargo run --example query
```

## API Reference

### Client

```rust
// Create client
let client = Client::new("http://localhost:8899");

// Or with custom configuration
let client = Client::builder()
    .rpc_url("http://localhost:8899")
    .timeout(Duration::from_secs(30))
    .build()?;

// Query methods
client.get_slot().await?;
client.get_balance(&pubkey).await?;
client.get_block(slot).await?;
client.get_latest_block().await?;
client.get_network_info().await?;
client.get_validators().await?;
```

### Keypair Management

```rust
// Generate new keypair
let keypair = Keypair::new();

// From seed
let seed = [0u8; 32];  // Use secure random seed
let keypair = Keypair::from_seed(&seed);

// Get public key
let pubkey = keypair.pubkey();
println!("Address: {}", pubkey.to_base58());

// Sign message
let message = b"Hello MoltChain";
let signature = keypair.sign(message);
```

### Transaction Building

```rust
use moltchain_sdk::{TransactionBuilder, Instruction, Hash};

// Build transaction
let tx = TransactionBuilder::new()
    .add_instruction(transfer_instruction)
    .recent_blockhash(blockhash)
    .build_and_sign(&keypair)?;

// Serialize and send
let tx_bytes = bincode::serialize(&tx)?;
let tx_base64 = base64::encode(&tx_bytes);
client.send_raw_transaction(&tx_base64).await?;
```

## Wallet Compatibility

### Solana Compatibility

MoltChain uses Ed25519 keypairs with Base58 encoding, making it compatible with Solana wallet formats:

- ✅ Phantom Wallet (import/export)
- ✅ Solflare Wallet (import/export)
- ✅ Solana CLI format

### File Format

Keypairs are saved in JSON format:

```json
{
  "privateKey": [/* 32 bytes */],
  "publicKey": [/* 32 bytes */],
  "publicKeyBase58": "3dmaXkMCpRn9wvD3wQNihjRPN3znnG9y56Xtq2drZZgU"
}
```

## Testing

```bash
# Run tests
cargo test

# Run with validator
cargo test -- --test-threads=1
```

## License

MIT OR Apache-2.0

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for guidelines.

## Resources

- [Documentation](https://developers.moltchain.network)
- [Examples](./examples/)
- [MoltChain CLI](../../cli/)
- [Python SDK](../python/)
