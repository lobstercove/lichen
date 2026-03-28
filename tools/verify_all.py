#!/usr/bin/env python3
"""Retry the first SELL at $0.100 that keeps disconnecting, then verify all pairs"""
import sys, os, struct, asyncio, json, urllib.request
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    keys = Path('data/state-testnet/genesis-keys')
    reserve = Keypair.load(keys / 'reserve_pool-lichen-testnet-1.json')
    caller_bytes = bytes(reserve.public_key().to_bytes())

    result = await conn._rpc("getAllSymbolRegistry")
    dex_core_addr = dex_amm_addr = None
    for e in result.get("entries", []):
        if e.get("symbol") == "DEX":
            dex_core_addr = e.get("program")
        elif e.get("symbol") == "DEXAMM":
            dex_amm_addr = e.get("program")
    dex_core = PublicKey.from_base58(dex_core_addr)

    # Retry SELL 700K LICN @ $0.100
    print("Retrying SELL 700,000 LICN @ $0.100...")
    price_spores = int(0.100 * SPORES)
    qty_spores = 700_000 * SPORES
    args = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', 1) + bytes([1]) + bytes([0]) +
        struct.pack('<Q', price_spores) +
        struct.pack('<Q', qty_spores) +
        struct.pack('<Q', 2_592_000)
    )
    try:
        sig = await call_contract_raw(conn, reserve, dex_core, 'call', list(args), value=qty_spores)
        print(f"  ✓ sig: {sig[:16]}...")
    except Exception as e:
        print(f"  Failed: {e}")

    await asyncio.sleep(1)

    # Verify all 7 pairs
    print("\n=== Order Book Verification ===")
    RPC = 'http://127.0.0.1:8899'
    pair_names = {1: "LICN/lUSD", 2: "wSOL/lUSD", 3: "wETH/lUSD", 4: "wSOL/LICN", 5: "wETH/LICN", 6: "wBNB/lUSD", 7: "wBNB/LICN"}
    all_good = True
    for pid in range(1, 8):
        payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getOrderBook", "params": [pid, 25]})
        # Use REST API
        req = urllib.request.Request(
            f'{RPC}/api/v1/pairs/{pid}/orderbook',
            headers={"Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req, timeout=10) as resp:
            d = json.loads(resp.read()).get("data", {})
        asks = d.get("asks", [])
        bids = d.get("bids", [])
        name = pair_names.get(pid, f"Pair {pid}")
        if asks and bids:
            ba = asks[0]['price']
            bb = bids[0]['price']
            sp = ba - bb
            mid = (ba + bb) / 2
            pct = sp / mid * 100 if mid else 0
            status = "✓" if pct < 10 else "⚠"
            print(f"  {status} {name:12s}  A:{len(asks):>2} B:{len(bids):>2}  ask={ba:<12.4f} bid={bb:<12.4f} spread={pct:.2f}%")
        elif not asks and not bids:
            print(f"  ✗ {name:12s}  EMPTY")
            all_good = False
        else:
            print(f"  ⚠ {name:12s}  A:{len(asks)} B:{len(bids)} — one-sided!")
            all_good = False

    # Verify AMM pools
    print("\n=== AMM Pool Verification ===")
    for pid in range(1, 8):
        req = urllib.request.Request(
            f'{RPC}/api/v1/pools/{pid}',
            headers={"Content-Type": "application/json"}
        )
        try:
            with urllib.request.urlopen(req, timeout=10) as resp:
                pd = json.loads(resp.read()).get("data", {})
            liq = pd.get("totalLiquidity", pd.get("liquidity", 0))
            print(f"  Pool {pid} ({pair_names.get(pid, '?')}): liquidity={liq}")
        except Exception as e:
            print(f"  Pool {pid}: {e}")

    if all_good:
        print("\n✓ ALL 7 PAIRS HAVE BOTH SIDES")
    else:
        print("\n✗ SOME PAIRS ARE MISSING ORDERS")

asyncio.run(main())
