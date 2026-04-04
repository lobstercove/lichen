#!/usr/bin/env python3
"""Test a single AMM add_liquidity call and check return code."""
import sys, os, struct, asyncio, json, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))

from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw, find_genesis_keypair_path

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

async def main():
    conn = Connection(RPC)
    rp_path = find_genesis_keypair_path('reserve_pool', NETWORK)
    reserve = Keypair.load(rp_path)
    print(f"Reserve: {reserve.public_key()}")

    r = await conn._rpc('getAllSymbolRegistry')
    entries = r.get('entries', [])
    dex_amm = None
    for e in entries:
        if e.get('symbol') == 'DEXAMM':
            dex_amm = PublicKey.from_base58(e['program'])
    print(f"dex_amm: {dex_amm}")

    # Pool 2: wSOL/lUSD - wrapped pair (no LICN involved)
    pool_id = 2
    lower_tick = 39960
    upper_tick = 47940
    amount_a = 1_000_000_000      # 1 wSOL
    amount_b = 100_000_000_000    # 100 lUSD

    caller_bytes = bytes(reserve.address().to_bytes())
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

    try:
        sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args))
        print(f"  Sig: {sig}")
    except Exception as ex:
        print(f"  SUBMIT FAILED: {ex}")
        return

    # Wait for inclusion with retries
    for attempt in range(15):
        await asyncio.sleep(2)
        try:
            tx = await conn._rpc('getTransaction', [sig])
            print(f"\n  TX FOUND after {(attempt+1)*2}s!")
            print(f"  Status: {tx.get('status')}")
            print(f"  Return code: {tx.get('return_code', tx.get('returnCode', '?'))}")
            # Print abbreviated tx details
            for key in ['type', 'slot', 'fee', 'contract_address']:
                if key in tx:
                    print(f"  {key}: {tx[key]}")
            logs = tx.get('logs', [])
            if logs:
                print(f"  Logs ({len(logs)}):")
                for log in logs[:5]:
                    print(f"    {log}")
            break
        except Exception as e:
            if attempt < 14:
                print(f"  Attempt {attempt+1}: {e}")
            else:
                print(f"\n  TX NEVER FOUND after 30s!")

    # Check pool liquidity
    try:
        r2 = urllib.request.urlopen(f'{RPC}/api/v1/pools/2').read()
        pool = json.loads(r2)
        data = pool.get('data', pool)
        print(f"\n  Pool 2 liquidity: {data.get('liquidity', '?')}")
    except Exception as e:
        print(f"\n  Pool check error: {e}")

asyncio.run(main())
