# Lichen JavaScript/TypeScript SDK

Official SDK for interacting with Lichen blockchain.

## Installation

```bash
npm install @lichen/sdk
```

## Quick Start

```typescript
import { Connection, Keypair, PublicKey } from '@lichen/sdk';

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

## Features

- ✅ Complete RPC client (24 endpoints)
- ✅ WebSocket subscriptions (real-time events)
- ✅ Native PQ keypairs and self-contained signatures
- ✅ Transaction builder
- ✅ TypeScript types
- ✅ Address and PQ public-key utilities
- ✅ Full blockchain interaction

## License

MIT
