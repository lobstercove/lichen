# Lichen Python SDK

Official Python SDK for interacting with Lichen blockchain.

## Installation

```bash
pip install lichen-sdk
```

## Quick Start

```python
import asyncio
from lichen import Connection, Keypair, PublicKey

async def main():
    # Connect to Lichen
    connection = Connection('http://localhost:8899')

    # Generate a native PQ keypair
    keypair = Keypair.generate()
    print(f"Address: {keypair.pubkey().to_base58()}")
    
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
- ✅ Native PQ keypairs and self-contained signatures
- ✅ Transaction builder
- ✅ Type hints throughout
- ✅ Address and PQ public-key utilities
- ✅ Full blockchain interaction

## Documentation

See the [full documentation](../../docs/SDK.md) for detailed API reference.

## License

MIT
