# Lichen Python SDK

Official Python SDK for interacting with Lichen blockchain.

Current source package version: `1.0.0`.

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

- ✅ Core async RPC client for chain, account, transaction, network, and restriction reads
- ✅ WebSocket subscriptions (real-time events)
- ✅ Native PQ keypairs and self-contained signatures
- ✅ Transaction builder
- ✅ Type hints throughout
- ✅ Address and PQ public-key utilities
- ✅ Full blockchain interaction

## Exchange Integration Helpers

Python preserves JSON integer precision, so raw spore values returned by RPC can
be handled exactly as `int` values. Exchange integrations should still use raw
spores, not formatted LICN strings.

Archive lookup helpers:

- `get_transaction(signature)`
- `get_block(slot)`
- `get_transactions_by_address(pubkey, limit=10, before_slot=None)`
- `get_transaction_history(pubkey, limit=10, before_slot=None)`
- `get_account_tx_count(pubkey)`

## Documentation

See the [Python SDK reference](https://developers.lichen.network/sdk-python.html) for detailed API reference.

## License

MIT
