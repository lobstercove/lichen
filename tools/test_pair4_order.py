#!/usr/bin/env python3
"""Test a single order on LICN-quoted pairs (4,5,7) with return code inspection"""
import sys, os, struct, asyncio, json, base64
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw, load_genesis_keypair

SPORES = 1_000_000_000
SIDE_BUY = 0
SIDE_SELL = 1
ORDER_LIMIT = 0
EXPIRY_SLOTS = 2_592_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    reserve = load_genesis_keypair('reserve_pool')

    result = await conn._rpc("getAllSymbolRegistry")
    dex_core_addr = None
    for e in result.get("entries", []):
        if e.get("symbol") == "DEX":
            dex_core_addr = e.get("program")
            break
    dex_core = PublicKey.from_base58(dex_core_addr)

    caller_bytes = bytes(reserve.address().to_bytes())

    # Test pair 4 (wSOL/LICN): SELL 50 wSOL at 840 LICN
    pair_id = 4
    price = 840.0  # LICN per wSOL
    qty = 50  # wSOL
    price_spores = int(price * SPORES)
    qty_spores = qty * SPORES

    print(f"=== Test SELL on pair {pair_id} (wSOL/LICN) ===")
    print(f"  Price: {price} LICN, Qty: {qty} wSOL")
    print(f"  price_spores: {price_spores}, qty_spores: {qty_spores}")

    args = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', pair_id) +
        bytes([SIDE_SELL]) +
        bytes([ORDER_LIMIT]) +
        struct.pack('<Q', price_spores) +
        struct.pack('<Q', qty_spores) +
        struct.pack('<Q', EXPIRY_SLOTS)
    )

    # SELL wSOL: base is NOT native, so no value needed. Escrow via transfer_from on wSOL
    try:
        sig = await call_contract_raw(conn, reserve, dex_core, 'call', list(args), value=0)
        print(f"  TX sig: {sig}")
    except Exception as e:
        print(f"  TX failed: {e}")
        return

    await asyncio.sleep(2)

    # Check the transaction to see return code
    r = await conn._rpc("getTransaction", [sig])
    rc = r.get("return_code", r.get("returnCode", "?"))
    logs = r.get("logs", [])
    status = r.get("status", "?")
    print(f"  Status: {status}, Return code: {rc}")
    if logs:
        for l in logs:
            print(f"  Log: {l}")

    # Now test BUY on pair 4
    print(f"\n=== Test BUY on pair {pair_id} (wSOL/LICN) ===")
    buy_price = 830.0
    buy_qty = 50

    bp_spores = int(buy_price * SPORES)
    bq_spores = buy_qty * SPORES

    # BUY wSOL with LICN (native) — need to send value
    notional = bp_spores * bq_spores // SPORES
    fee = max(notional * 5 // 10_000, 1)
    value = notional + fee
    print(f"  Price: {buy_price} LICN, Qty: {buy_qty} wSOL")
    print(f"  Notional: {notional / SPORES:.2f} LICN, Fee: {fee / SPORES:.6f} LICN")
    print(f"  Value to send: {value / SPORES:.2f} LICN")

    args2 = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', pair_id) +
        bytes([SIDE_BUY]) +
        bytes([ORDER_LIMIT]) +
        struct.pack('<Q', bp_spores) +
        struct.pack('<Q', bq_spores) +
        struct.pack('<Q', EXPIRY_SLOTS)
    )

    try:
        sig2 = await call_contract_raw(conn, reserve, dex_core, 'call', list(args2), value=value)
        print(f"  TX sig: {sig2}")
    except Exception as e:
        print(f"  TX failed: {e}")
        return

    await asyncio.sleep(2)
    r2 = await conn._rpc("getTransaction", [sig2])
    rc2 = r2.get("return_code", r2.get("returnCode", "?"))
    logs2 = r2.get("logs", [])
    status2 = r2.get("status", "?")
    print(f"  Status: {status2}, Return code: {rc2}")
    if logs2:
        for l in logs2:
            print(f"  Log: {l}")

    # Check order book after
    print(f"\n=== Pair 4 order book after ===")
    import urllib.request
    req = urllib.request.Request(
        'http://127.0.0.1:8899/api/v1/pairs/4/orderbook',
        headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read()).get("data", {})
        asks = data.get("asks", [])
        bids = data.get("bids", [])
        print(f"  Asks: {len(asks)}, Bids: {len(bids)}")
        for a in asks[:3]:
            print(f"    ASK {a['quantity']/1e9:.0f} @ {a['price']}")
        for b in bids[:3]:
            print(f"    BID {b['quantity']/1e9:.0f} @ {b['price']}")

asyncio.run(main())
