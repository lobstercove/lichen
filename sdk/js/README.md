# Lichen JavaScript/TypeScript SDK

Official SDK for interacting with Lichen blockchain.

## Installation

```bash
npm install @lichen/sdk
```

## Quick Start

```typescript
import { Connection, PublicKey } from '@lichen/sdk';

// Connect to Lichen
const connection = new Connection('http://localhost:8899');

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

See the [full documentation](../../docs/SDK.md) for detailed API reference.

## Features

- ✅ Complete RPC client (24 endpoints)
- ✅ WebSocket subscriptions (real-time events)
- ✅ Transaction builder
- ✅ TypeScript types
- ✅ PublicKey utilities
- ✅ Full blockchain interaction

## License

MIT
