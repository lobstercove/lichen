#!/usr/bin/env python3
"""Verify DEX orders and AMM pools on-chain."""
import sys, os, base64, struct, asyncio
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

SPORES = 1_000_000_000

PAIR_NAMES = {
    1: "LICN/lUSD", 2: "wSOL/lUSD", 3: "wETH/lUSD",
    4: "wSOL/LICN", 5: "wETH/LICN", 6: "wBNB/lUSD", 7: "wBNB/LICN",
}

async def main():
    conn = Connection('http://127.0.0.1:8899')
    keys = Path('data/state-testnet/genesis-keys')
    reserve = Keypair.load(keys / 'reserve_pool-lichen-testnet-1.json')
    reserve_str = str(reserve.public_key())

    # Find DEX contract
    result = await conn._rpc("getAllSymbolRegistry")
    dex_addr = None
    dex_amm_addr = None
    token_addrs = {}
    for e in result.get("entries", []):
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym == "DEX":
            dex_addr = prog
        elif sym == "DEXAMM":
            dex_amm_addr = prog
        elif sym in ("LUSD", "WSOL", "WETH", "WBNB"):
            token_addrs[sym] = prog

    print(f"DEX Core: {dex_addr}")
    print(f"DEX AMM:  {dex_amm_addr}")
    print()

    # Query order book for each pair using opcode 10 (get_order_book)
    # Args: opcode(1) + pair_id(u64)
    for pair_id in range(1, 8):
        name = PAIR_NAMES[pair_id]
        args = bytes([10]) + struct.pack('<Q', pair_id)
        args_b64 = base64.b64encode(args).decode()
        try:
            r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
            rc = r.get('returnCode', 0)
            rd = r.get('returnData', None)
            logs = r.get('logs', [])
            success = r.get('success', False)
            print(f"  {name:12s} (pair {pair_id}): returnCode={rc}, success={success}, logs={len(logs)}")
            if rd:
                print(f"    returnData: {rd[:80]}...")
            if logs:
                for log in logs[:5]:
                    print(f"    log: {log}")
        except Exception as e:
            print(f"  {name:12s} (pair {pair_id}): ERROR {e}")

    print()
    
    # Also try querying user orders - opcode 11 (get_user_orders)
    # Args: opcode(1) + trader(32B)
    reserve_bytes = bytes(reserve.public_key().to_bytes())
    args = bytes([11]) + reserve_bytes
    args_b64 = base64.b64encode(args).decode()
    try:
        r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
        rc = r.get('returnCode', 0)
        rd = r.get('returnData', None)
        logs = r.get('logs', [])
        print(f"  User orders: returnCode={rc}, logs={len(logs)}")
        if logs:
            for log in logs[:10]:
                print(f"    {log}")
    except Exception as e:
        print(f"  User orders query: ERROR {e}")
    
    print()
    
    # Check token balances after seeding
    print("Token balances (reserve_pool) after seeding:")
    reserve_bytes = bytes(reserve.public_key().to_bytes())
    args_b64 = base64.b64encode(reserve_bytes).decode()
    for sym, addr in sorted(token_addrs.items()):
        r = await conn._rpc('callContract', [addr, 'balance_of', args_b64, reserve_str])
        rc = r.get('returnCode', 0)
        print(f"  {sym:5s}: {rc / SPORES:>15,.3f} tokens")

    # Check native LICN balance
    acct = await conn._rpc('getAccountInfo', [reserve_str])
    balance = acct.get('balance', 0)
    print(f"  LICN : {balance / SPORES:>15,.3f} native")

asyncio.run(main())
