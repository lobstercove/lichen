#!/usr/bin/env python3
"""Check AMM pool liquidity and LP positions after seeding."""
import sys, os, json, urllib.parse, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Keypair
from chain_helpers import load_genesis_keypair

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

EXPECTED_POOLS = {
    1: "LICN/lUSD",
    2: "wSOL/lUSD",
    3: "wETH/lUSD",
    4: "wSOL/LICN",
    5: "wETH/LICN",
    6: "wBNB/lUSD",
    7: "wBNB/LICN",
    8: "wNEO/lUSD",
    9: "wNEO/LICN",
    10: "wGAS/lUSD",
    11: "wGAS/LICN",
    12: "wBTC/lUSD",
    13: "wBTC/LICN",
}

reserve = load_genesis_keypair('reserve_pool', NETWORK)
reserve_hex = reserve.address().to_bytes().hex()
reserve_address = str(reserve.address())

print(f"Reserve: {reserve_address}")
print(f"Reserve hex: {reserve_hex}")

# Check all pools
print("\n=== AMM Pools ===")
r = urllib.request.urlopen(f'{RPC}/api/v1/pools').read()
pools = json.loads(r)
seen = {}
for p in pools.get('data', []):
    pid = p['poolId']
    liq = p['liquidity']
    ta = p.get('tokenASymbol','?')
    tb = p.get('tokenBSymbol','?')
    price = p.get('price', 0)
    seen[int(pid)] = int(liq)
    status = "✅ HAS LIQ" if liq > 0 else "❌ EMPTY"
    print(f"  Pool {pid}: {ta}/{tb}  liq={liq:>20,}  {status}")

missing = sorted(set(EXPECTED_POOLS) - set(seen))
empty = sorted(pid for pid in EXPECTED_POOLS if seen.get(pid, 0) <= 0)
if missing:
    print(f"\nMissing expected pools: {', '.join(f'{pid} {EXPECTED_POOLS[pid]}' for pid in missing)}")
if empty:
    print(f"\nEmpty expected pools: {', '.join(f'{pid} {EXPECTED_POOLS[pid]}' for pid in empty)}")

print("\n=== LP Positions ===")
positions_url = f"{RPC}/api/v1/pools/positions?owner={urllib.parse.quote(reserve_address, safe='')}"
positions_payload = json.loads(urllib.request.urlopen(positions_url).read())
positions = positions_payload.get('data', []) if positions_payload.get('success') else []
print(f"  Positions found: {len(positions)}")
for pos in positions:
    print(
        f"  Position {pos['positionId']}: pool={pos['poolId']} ticks=[{pos['lowerTick']}, {pos['upperTick']}] liq={pos['liquidity']:,}"
    )

if missing or empty:
    sys.exit(1)
