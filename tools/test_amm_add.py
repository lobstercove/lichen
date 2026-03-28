#!/usr/bin/env python3
"""Test add_liquidity on pool 2 (wSOL/lUSD) to diagnose return code."""
import sys, os, struct, asyncio, json, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))

from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')

async def main():
    conn = Connection(RPC)

    rp_path = Path('data/state-testnet/genesis-keys/reserve_pool-lichen-testnet-1.json')
    reserve = Keypair.load(rp_path)
    print(f"Reserve: {reserve.public_key()}")

    # Find dex_amm
    r = await conn._rpc('getAllSymbolRegistry')
    entries = r.get('entries', [])
    dex_amm = None
    for e in entries:
        if e.get('symbol') == 'DEXAMM':
            dex_amm = PublicKey.from_base58(e['program'])
            break
    print(f"dex_amm: {dex_amm}")
    if not dex_amm:
        print("ERROR: dex_amm not found")
        return

    # Pool 2: wSOL/lUSD, tick=44278, tick_spacing=60
    pool_id = 2
    lower_tick = 39960   # round(40000/60)*60
    upper_tick = 47940   # round(48000/60)*60
    amount_a = 1_000_000_000     # 1 wSOL
    amount_b = 100_000_000_000   # 100 lUSD

    caller_bytes = bytes(reserve.public_key().to_bytes())
    args = (
        bytes([3]) +
        caller_bytes +
        struct.pack('<Q', pool_id) +
        struct.pack('<i', lower_tick) +
        struct.pack('<i', upper_tick) +
        struct.pack('<Q', amount_a) +
        struct.pack('<Q', amount_b)
    )

    print(f"\nadd_liquidity pool={pool_id} ticks=[{lower_tick},{upper_tick}] a={amount_a} b={amount_b}")
    try:
        sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args))
        print(f"Sig: {sig}")
    except Exception as e:
        print(f"FAILED: {e}")
        return

    await asyncio.sleep(1.5)

    # Get tx details
    try:
        tx = await conn._rpc('getTransaction', [sig])
        result = tx if isinstance(tx, dict) else {}
        print(f"returnCode: {result.get('returnCode', '?')}")
        logs = result.get('logs', [])
        for log in logs:
            print(f"  log: {log}")
    except Exception as e:
        print(f"getTransaction error: {e}")

    # Check pool 2
    r = urllib.request.urlopen(f'{RPC}/api/v1/pools/2').read()
    pool = json.loads(r)
    print(f"\nPool 2 liquidity: {pool.get('data',{}).get('liquidity', '?')}")

    # Check positions for reserve
    reserve_hex = reserve.public_key().to_bytes().hex()
    r = urllib.request.urlopen(f'{RPC}/api/v1/positions/{reserve_hex}').read()
    pos = json.loads(r)
    print(f"Positions: {json.dumps(pos)[:200]}")

asyncio.run(main())
