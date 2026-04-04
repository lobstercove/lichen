#!/usr/bin/env python3
"""Diagnose why buy orders are not appearing on the LICN/lUSD order book."""
import sys, os, base64, struct, asyncio
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw, load_genesis_keypair

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    reserve = load_genesis_keypair('reserve_pool')
    reserve_str = str(reserve.address())
    reserve_bytes = bytes(reserve.address().to_bytes())

    # Discover contracts
    result = await conn._rpc("getAllSymbolRegistry")
    contracts = {}
    for e in result.get("entries", []):
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym in ("DEX", "DEXAMM", "LUSD") and prog:
            contracts[sym] = prog

    dex_addr = contracts["DEX"]
    lusd_addr = contracts["LUSD"]
    dex_pk = PublicKey.from_base58(dex_addr)
    lusd_pk = PublicKey.from_base58(lusd_addr)

    print(f"Reserve:  {reserve_str}")
    print(f"DEX:      {dex_addr}")
    print(f"lUSD:     {lusd_addr}")

    # 1. Check reserve_pool lUSD balance
    args_b64 = base64.b64encode(reserve_bytes).decode()
    r = await conn._rpc('callContract', [lusd_addr, 'balance_of', args_b64, reserve_str])
    lusd_balance = r.get('returnCode', 0)
    print(f"\nlUSD balance: {lusd_balance / SPORES:,.3f} tokens ({lusd_balance:,} spores)")

    # 2. Check total lUSD supply
    r = await conn._rpc('callContract', [lusd_addr, 'total_supply', '', reserve_str])
    total_supply = r.get('returnCode', 0)
    print(f"lUSD total supply: {total_supply / SPORES:,.3f} tokens")

    # 3. Check lUSD allowance: reserve_pool → dex_core
    # allowance(owner[32B], spender[32B])
    dex_bytes = bytes(dex_pk.to_bytes())
    allowance_args = reserve_bytes + dex_bytes
    args_b64 = base64.b64encode(allowance_args).decode()
    r = await conn._rpc('callContract', [lusd_addr, 'allowance', args_b64, reserve_str])
    allowance = r.get('returnCode', 0)
    rd = r.get('returnData', None)
    print(f"lUSD allowance (reserve→dex): returnCode={allowance}, returnData={rd}")
    if allowance > 0:
        print(f"  = {allowance / SPORES:,.3f} tokens")
    else:
        print(f"  *** ZERO allowance! This is why buy orders fail. ***")

    # 4. Check order book state: best bid and best ask for pair 1
    # opcode 10 = get_order_book
    ob_args = bytes([10]) + struct.pack('<Q', 1)
    args_b64 = base64.b64encode(ob_args).decode()
    r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
    print(f"\nOrder book (pair 1): returnCode={r.get('returnCode',0)}, returnData={r.get('returnData','')}")

    # 5. Check pair count
    pc_args = bytes([5])
    args_b64 = base64.b64encode(pc_args).decode()
    r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
    print(f"Pair count: returnCode={r.get('returnCode',0)}")

    # 6. Simulate a buy order via callContract (read-only)
    # opcode 2, pair 1, side BUY, order_type LIMIT, price=0.098, qty=1000
    price = int(0.098 * SPORES)
    qty = 1000 * SPORES
    order_args = (
        bytes([2]) +
        reserve_bytes +
        struct.pack('<Q', 1) +      # pair_id
        bytes([0]) +                # side = BUY
        bytes([0]) +                # order_type = LIMIT
        struct.pack('<Q', price) +  # price
        struct.pack('<Q', qty) +    # quantity
        struct.pack('<Q', 2592000)  # expiry
    )
    args_b64 = base64.b64encode(order_args).decode()
    r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
    rc = r.get('returnCode', 0)
    rd = r.get('returnData', None)
    logs = r.get('logs', [])
    print(f"\nSimulated BUY order (pair 1, $0.098, 1000 LICN):")
    print(f"  returnCode={rc}")
    print(f"  returnData={rd}")
    print(f"  logs={logs}")
    
    # Return code meaning:
    # 0 = success
    # 4 = invalid price/quantity (tick/lot alignment)
    # 10 = price outside oracle band
    # 11 = escrow failed
    rc_meanings = {
        0: "SUCCESS", 1: "not initialized", 2: "not admin", 3: "pair not found",
        4: "invalid price/qty (tick/lot)", 5: "user limit", 6: "reentrancy",
        7: "post-only would match", 8: "pair paused", 9: "expired",
        10: "price outside oracle band", 11: "ESCROW FAILED (balance/allowance)",
        12: "reduce-only failed"
    }
    print(f"  meaning: {rc_meanings.get(rc, f'UNKNOWN ({rc})')}")

    # 7. Also try a sell order simulation
    sell_price = int(0.100 * SPORES)
    sell_qty = 1000 * SPORES
    sell_args = (
        bytes([2]) +
        reserve_bytes +
        struct.pack('<Q', 1) +
        bytes([1]) +                # SELL
        bytes([0]) +
        struct.pack('<Q', sell_price) +
        struct.pack('<Q', sell_qty) +
        struct.pack('<Q', 2592000)
    )
    args_b64 = base64.b64encode(sell_args).decode()
    r = await conn._rpc('callContract', [dex_addr, 'call', args_b64, reserve_str])
    rc = r.get('returnCode', 0)
    print(f"\nSimulated SELL order (pair 1, $0.100, 1000 LICN):")
    print(f"  returnCode={rc}")
    print(f"  meaning: {rc_meanings.get(rc, f'UNKNOWN ({rc})')}")

    # 8. Check if reserve has enough native LICN
    acct = await conn._rpc('getAccountInfo', [reserve_str])
    native = acct.get('balance', 0)
    print(f"\nNative LICN balance: {native / SPORES:,.3f} LICN")

asyncio.run(main())
