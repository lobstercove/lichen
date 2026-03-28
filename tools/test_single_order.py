#!/usr/bin/env python3
"""Test a single LICN/lUSD sell order on the CLOB."""
import sys, os, struct, asyncio
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')

async def main():
    conn = Connection(RPC)

    # Load reserve_pool keypair
    reserve = Keypair.load(Path("data/state-testnet/genesis-keys/reserve_pool-lichen-testnet-1.json"))
    print(f"Reserve: {reserve.public_key()}")

    # Discover dex_core
    result = await conn._rpc("getAllSymbolRegistry")
    entries = result.get("entries", [])
    dex_core = None
    for e in entries:
        if e.get("symbol") == "DEX" and e.get("program"):
            dex_core = PublicKey.from_base58(e["program"])
            break
    if not dex_core:
        print("ERROR: dex_core not found in symbol registry")
        return
    print(f"dex_core: {dex_core}")

    # Test: one sell order LICN/lUSD @ $0.100, 1000 LICN
    caller_bytes = bytes(reserve.public_key().to_bytes())
    pair_id = 1  # LICN/lUSD
    SIDE_SELL = 1
    ORDER_LIMIT = 0
    EXPIRY_SLOTS = 2_592_000

    price_spores = int(0.100 * SPORES)
    qty_spores = 1000 * SPORES

    args = (
        bytes([2])                                +  # opcode 2
        caller_bytes                              +  # trader (32B)
        struct.pack('<Q', pair_id)                +  # pair_id
        bytes([SIDE_SELL])                        +  # side
        bytes([ORDER_LIMIT])                      +  # order_type
        struct.pack('<Q', price_spores)           +  # price
        struct.pack('<Q', qty_spores)             +  # quantity
        struct.pack('<Q', EXPIRY_SLOTS)              # expiry
    )

    sig = await call_contract_raw(conn, reserve, dex_core, "call", list(args))
    print(f"SELL 1000 LICN @ $0.100 -> sig: {sig}")

asyncio.run(main())
