# Lichen Python SDK

Official Python SDK for interacting with Lichen blockchain.

## Installation

```bash
pip install lichen-sdk
```

## Quick Start

```python
import asyncio
from lichen import Connection, PublicKey

async def main():
    # Connect to Lichen
    connection = Connection('http://localhost:8899')
    
    # Get account balance
    pubkey = PublicKey('YourPublicKeyHere...')
    balance = await connection.get_balance(pubkey)
    print(f"Balance: {balance['licn']} LICN")
    
    # Subscribe to blocks
    async def on_block(block):
        print(f"New block: {block}")
    
    await connection.on_block(on_block)

asyncio.run(main())
```

## Features

- ✅ Complete async RPC client (24 endpoints)
- ✅ WebSocket subscriptions (real-time events)
- ✅ Transaction builder
- ✅ Type hints throughout
- ✅ PublicKey utilities
- ✅ Full blockchain interaction

## Documentation

See the [full documentation](../../docs/SDK.md) for detailed API reference.

## License

MIT
