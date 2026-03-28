#!/usr/bin/env python3
"""Direct test: add_liquidity to pool 2 (wSOL/lUSD) with return code check."""
import sys, os, struct, asyncio, json, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))

from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

async def main():
    conn = Connection(RPC)

    rp_path = Path(f'data/state-{NETWORK}/genesis-keys/reserve_pool-lichen-{NETWORK}-1.json')
    reserve = Keypair.load(rp_path)
    print(f"Reserve: {reserve.public_key()}")

    # Find dex_amm
    r = await conn._rpc('getAllSymbolRegistry')
    entries = r.get('entries', [])
    dex_amm = None
    for e in entries:
        if e.get('symbol') == 'DEXAMM':
            dex_amm = PublicKey.from_base58(e['program'])
    print(f"dex_amm: {dex_amm}")

    # Pool 2: wSOL/lUSD, tick=44278 (approx), tick_spacing=60
    pool_id = 2
    lower_tick = 39960
    upper_tick = 47940
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

    print(f"\nadd_liquidity pool={pool_id} ticks=[{lower_tick},{upper_tick}]")
    print(f"  amount_a={amount_a} amount_b={amount_b}")
    print(f"  args len = {len(list(args))} (expected: 1+32+8+4+4+8+8 = 65)")
    
    try:
        sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args))
        print(f"  Sig: {sig}")
    except Exception as e:
        print(f"  FAILED: {e}")
        return

    await asyncio.sleep(2)

    # Get tx details
    try:
        tx = await conn._rpc('getTransaction', [sig])
        print(f"\n  Transaction details:")
        print(f"    returnCode: {tx.get('returnCode', '?')}")
        print(f"    success: {tx.get('success', '?')}")
        for log in tx.get('logs', []):
            print(f"    log: {log}")
    except Exception as e:
        print(f"  getTransaction: {e}")

    # Check pool 2
    r = urllib.request.urlopen(f'{RPC}/api/v1/pools/2').read()
    pool = json.loads(r)
    data = pool.get('data', pool)
    print(f"\n  Pool 2 liquidity: {data.get('liquidity', '?')}")

asyncio.run(main())
