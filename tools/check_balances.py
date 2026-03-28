#!/usr/bin/env python3
"""Check token balances of reserve_pool."""
import sys, os, struct, asyncio, base64
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
from lichen import Connection, Keypair, PublicKey

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

async def main():
    conn = Connection(RPC)
    keys = Path(__file__).resolve().parent.parent / f'data/state-{NETWORK}/genesis-keys'
    reserve = Keypair.load(keys / f'reserve_pool-lichen-{NETWORK}-1.json')
    reserve_bytes = bytes(reserve.public_key().to_bytes())

    # LICN balance
    bal = await conn._rpc('getBalance', [str(reserve.public_key())])
    spores = bal.get('spores', 0) if isinstance(bal, dict) else bal
    print(f"Reserve pool: {reserve.public_key()}")
    print(f"  LICN: {spores / SPORES:,.3f}")

    # Token balances
    result = await conn._rpc("getAllSymbolRegistry")
    entries = result.get("entries", [])
    for e in entries:
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym in ("LUSD", "WSOL", "WETH", "WBNB") and prog:
            args_b64 = base64.b64encode(reserve_bytes).decode()
            r = await conn._rpc("callContract", [prog, "balance_of", args_b64, str(reserve.public_key())])
            if r.get("success") and r.get("returnData"):
                data = base64.b64decode(r["returnData"])
                balance = struct.unpack('<Q', data[:8])[0] if len(data) >= 8 else 0
                print(f"  {sym}: {balance / SPORES:,.3f}")
            else:
                print(f"  {sym}: query failed — {r.get('error', 'unknown')}")

asyncio.run(main())
