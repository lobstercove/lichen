# Lichen JavaScript/TypeScript SDK

Official SDK for interacting with Lichen blockchain.

Current source package version: `1.0.7`.

## Installation

```bash
npm install @lobstercove/lichen-sdk
```

## Quick Start

```typescript
import { Connection, Keypair, PublicKey } from '@lobstercove/lichen-sdk';

// Connect to Lichen
const connection = new Connection('http://localhost:8899');

// Generate a native PQ keypair
const keypair = Keypair.generate();
console.log('Address:', keypair.pubkey().toBase58());

// Get account balance
const pubkey = new PublicKey('YourPublicKeyHere...');
const balance = await connection.getBalance(pubkey);
console.log(`Balance: ${balance.licn} LICN`);

// Subscribe to blocks
connection.onBlock((block) => {
  console.log('New block:', block);
});
```

## Documentation

See the [JavaScript SDK reference](https://developers.lichen.network/sdk-js.html) for detailed API reference.

## Exchange Accounting Boundary

The JavaScript SDK uses native `JSON.parse` through `response.json()`. Large
u64 spore values can exceed `Number.MAX_SAFE_INTEGER`, so this package is not
approved for exchange accounting of raw balances or transfer amounts until a
lossless integer parser is added. For exchange deposit, withdrawal, and
reconciliation accounting, use Rust/Python SDK paths or a raw JSON-RPC client
with lossless integer handling.

The SDK does expose archive lookup helpers:

- `getTransaction(signature)`
- `getBlock(slot)`
- `getTransactionsByAddress(pubkey, limit, beforeSlot)`
- `getAccountTxCount(pubkey)`

## Features

- ✅ Core RPC client for chain, account, transaction, network, restriction, and Neo route reads
- ✅ WebSocket subscriptions (real-time events)
- ✅ Native PQ keypairs and self-contained signatures
- ✅ Transaction builder
- ✅ TypeScript types
- ✅ Address and PQ public-key utilities
- ✅ Neo X route, rewards, and reserve/liability proof helpers
- ✅ Full blockchain interaction

## License

MIT
