#!/usr/bin/env python3
"""Check AMM pool liquidity and LP positions after seeding."""
import sys, os, json, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Keypair

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

repo = Path(__file__).resolve().parent.parent
rp_path = repo / f'data/state-{NETWORK}/genesis-keys/reserve_pool-lichen-{NETWORK}-1.json'
reserve = Keypair.load(rp_path)
reserve_hex = reserve.public_key().to_bytes().hex()

print(f"Reserve: {reserve.public_key()}")
print(f"Reserve hex: {reserve_hex}")

# Check all pools
print("\n=== AMM Pools ===")
r = urllib.request.urlopen(f'{RPC}/api/v1/pools').read()
pools = json.loads(r)
for p in pools.get('data', []):
    pid = p['poolId']
    liq = p['liquidity']
    ta = p.get('tokenASymbol','?')
    tb = p.get('tokenBSymbol','?')
    price = p.get('price', 0)
    status = "✅ HAS LIQ" if liq > 0 else "❌ EMPTY"
    print(f"  Pool {pid}: {ta}/{tb}  liq={liq:>20,}  {status}")

# Check position count via RPC
print("\n=== LP Positions (RPC) ===")
import asyncio
sys.path.insert(0, os.path.join(os.path.dirname(__file__)))
from lichen import Connection

async def check_positions():
    conn = Connection(RPC)
    # Try getting positions via RPC
    try:
        result = await conn._rpc('getPositions', [str(reserve.public_key())])
        print(f"  getPositions result: {json.dumps(result)[:400]}")
    except Exception as e:
        print(f"  getPositions error: {e}")

    # Try getting via the pool positions endpoint
    for pid in range(1, 8):
        try:
            result = await conn._rpc('getPoolPositions', [pid])
            print(f"  Pool {pid} positions: {json.dumps(result)[:200]}")
        except Exception as e:
            print(f"  Pool {pid} positions error: {e}")

asyncio.run(check_positions())
