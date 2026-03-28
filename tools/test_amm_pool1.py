#!/usr/bin/env python3
"""Test AMM add_liquidity for pool 1 (LICN/lUSD) with native LICN value."""
import sys, os, struct, asyncio, json, urllib.request
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))

from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')
SPORES = 1_000_000_000

async def main():
    conn = Connection(RPC)
    rp_path = Path(f'data/state-{NETWORK}/genesis-keys/reserve_pool-lichen-{NETWORK}-1.json')
    reserve = Keypair.load(rp_path)
    print(f"Reserve: {reserve.public_key()}")

    r = await conn._rpc('getAllSymbolRegistry')
    entries = r.get('entries', [])
    dex_amm = None
    for e in entries:
        if e.get('symbol') == 'DEXAMM':
            dex_amm = PublicKey.from_base58(e['program'])
    print(f"dex_amm: {dex_amm}")

    # Pool 1: LICN/lUSD - native LICN as token_a
    pool_id = 1
    lower_tick = -30000  # aligned to 60
    upper_tick = -13860  # aligned to 60
    amount_a = 100 * SPORES     # 100 LICN (small test)
    amount_b = 10 * SPORES      # 10 lUSD

    # Value = native LICN spores for token_a
    value = amount_a

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
    print(f"  amount_a={amount_a} ({amount_a / SPORES} LICN)")
    print(f"  amount_b={amount_b} ({amount_b / SPORES} lUSD)")
    print(f"  value={value} spores ({value / SPORES} LICN)")

    try:
        sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args), value=value)
        print(f"  Sig: {sig}")
    except Exception as ex:
        print(f"  SUBMIT FAILED: {ex}")
        return

    for attempt in range(15):
        await asyncio.sleep(2)
        try:
            tx = await conn._rpc('getTransaction', [sig])
            print(f"\n  TX FOUND after {(attempt+1)*2}s!")
            print(f"  Status: {tx.get('status')}")
            rc = tx.get('return_code', tx.get('returnCode', '?'))
            print(f"  Return code: {rc}")
            for key in ['type', 'slot', 'fee']:
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

    # Check pool liquidity via REST
    try:
        r2 = urllib.request.urlopen(f'{RPC}/api/v1/pools/1').read()
        pool = json.loads(r2)
        data = pool.get('data', pool)
        print(f"\n  Pool 1 liquidity: {data.get('liquidity', '?')}")
    except Exception as e:
        print(f"\n  Pool 1 check error: {e}")

asyncio.run(main())
