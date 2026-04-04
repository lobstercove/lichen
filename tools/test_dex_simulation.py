#!/usr/bin/env python3
"""Test DEX order with proper value/escrow handling."""
import sys, os, struct, asyncio, json
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw, find_genesis_keypair_path

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')


async def main():
    conn = Connection(RPC)
    repo = Path(__file__).resolve().parent.parent

    # Load reserve_pool keypair
    rp_path = find_genesis_keypair_path('reserve_pool', NETWORK)
    reserve = Keypair.load(rp_path)
    print(f"Reserve: {reserve.public_key()}")

    # Discover dex_core and lusd from registry
    result = await conn._rpc("getAllSymbolRegistry")
    dex_core = lusd = None
    for e in result.get("entries", []):
        sym, prog = e.get("symbol", ""), e.get("program", "")
        if sym == "DEX" and prog:
            dex_core = PublicKey.from_base58(prog)
        elif sym == "LUSD" and prog:
            lusd = PublicKey.from_base58(prog)
    print(f"dex_core: {dex_core}")
    print(f"lusd:     {lusd}")

    caller_bytes = bytes(reserve.address().to_bytes())

    # Step 1: Approve DEX to spend reserve_pool's lUSD (for BUY side)
    print("\n--- Step 1: Approve lUSD for DEX ---")
    dex_bytes = bytes(dex_core.to_bytes())
    approve_args = list(caller_bytes + dex_bytes + struct.pack('<Q', 2**63 - 1))
    try:
        sig = await call_contract_raw(conn, reserve, lusd, "approve", approve_args)
        print(f"  lUSD approve sig: {sig}")
    except Exception as e:
        print(f"  lUSD approve FAILED: {e}")

    # Step 2: SELL 100 LICN @ $0.100 with value = 100 LICN
    print("\n--- Step 2: SELL 100 LICN @ $0.100 (with value) ---")
    qty = 100 * SPORES
    price = int(0.100 * SPORES)
    args = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', 1) +     # pair_id = LICN/lUSD
        bytes([1]) +               # side = SELL
        bytes([0]) +               # type = LIMIT
        struct.pack('<Q', price) +
        struct.pack('<Q', qty) +
        struct.pack('<Q', 2592000) # expiry
    )
    try:
        sig = await call_contract_raw(conn, reserve, dex_core, 'call', list(args), value=qty)
        print(f"  SELL sig: {sig}")
    except Exception as e:
        print(f"  SELL FAILED: {e}")
        return

    # Step 3: Wait and verify
    print("\n--- Step 3: Verify transaction ---")
    await asyncio.sleep(3)
    try:
        result = await conn._rpc("getTransaction", [sig])
        status = result.get("status", "?")
        slot = result.get("slot", "?")
        print(f"  status: {status}, slot: {slot}")
    except Exception as e:
        print(f"  getTransaction: {e}")

    # Step 4: BUY 100 LICN @ $0.098 (escrow lUSD via transfer_from)
    print("\n--- Step 4: BUY 100 LICN @ $0.098 (lUSD escrow) ---")
    buy_qty = 100 * SPORES
    buy_price = int(0.098 * SPORES)
    args2 = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', 1) +     # pair_id = LICN/lUSD
        bytes([0]) +               # side = BUY
        bytes([0]) +               # type = LIMIT
        struct.pack('<Q', buy_price) +
        struct.pack('<Q', buy_qty) +
        struct.pack('<Q', 2592000) # expiry
    )
    try:
        sig2 = await call_contract_raw(conn, reserve, dex_core, 'call', list(args2), value=0)
        print(f"  BUY sig: {sig2}")
    except Exception as e:
        print(f"  BUY FAILED: {e}")
        return

    # Step 5: Verify BUY
    print("\n--- Step 5: Verify BUY transaction ---")
    await asyncio.sleep(3)
    try:
        result = await conn._rpc("getTransaction", [sig2])
        status = result.get("status", "?")
        slot = result.get("slot", "?")
        print(f"  status: {status}, slot: {slot}")
    except Exception as e:
        print(f"  getTransaction: {e}")

    print("\nDone!")


asyncio.run(main())
