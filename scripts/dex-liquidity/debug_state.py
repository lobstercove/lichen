#!/usr/bin/env python3
"""Debug script to investigate contract state after liquidity operations."""
import asyncio
import sys
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))
from lichen import Connection, PublicKey

RPC = "http://15.204.229.189:8899"

async def main():
    conn = Connection(RPC)

    # 1. Admin vs deployer check
    deployer_pk = PublicKey.from_base58("3bAsAVCAE2gaSSC2P7pbKre4yvnE1zppgKmd7aggYou4")
    deployer_hex = deployer_pk.to_bytes().hex()
    stored_admin = "2678468f7792db92dee22c2541c6a0a176f85e36cd6946bf059286952d20f52b"
    print("=== ADMIN CHECK ===")
    print(f"  Deployer hex: {deployer_hex}")
    print(f"  Admin hex:    {stored_admin}")
    print(f"  Match: {deployer_hex == stored_admin}")
    print()

    # 2. lUSD storage
    lusd_addr = "Aw9NxRCw5P5SNiVzjNt31rw78vt91o8uPwa1MMCjLNbh"
    print("=== lUSD STORAGE ===")
    st = await conn._rpc("getProgramStorage", [lusd_addr])
    for e in st.get("entries", []):
        kd = e.get("key_decoded", e.get("key", ""))
        vh = e.get("value_hex", "")
        print(f"  {kd:30s} = {vh}")
    print()

    # 3. DEX storage
    dex_addr = "2W6vnnuHf5ohvgJuwRsm1T6ZgZ1UXm73QTiQnYe9dv8j"
    print("=== DEX STORAGE ===")
    dst = await conn._rpc("getProgramStorage", [dex_addr])
    entries = dst.get("entries", [])
    print(f"  Total entries: {len(entries)}")
    for e in entries[:40]:
        kd = e.get("key_decoded", e.get("key", ""))
        vh = e.get("value_hex", "")
        sz = e.get("size", 0)
        print(f"  {kd:40s} [{sz:>4}B] = {vh[:80]}")
    print()

    # 4. Mint TX details
    mint_sig = "813c6fc8110e09c018b9a90d691bb172f66ca871ba2a7d4c121d6cb0a2885ecc"
    print("=== MINT TX ===")
    tx = await conn._rpc("getTransaction", [mint_sig])
    for k in ["return_code", "return_data", "contract_logs", "compute_units", "status", "error"]:
        print(f"  {k}: {tx.get(k, 'MISSING')}")
    print()

    # 5. Check first sell order TX
    # Get recent transactions to find sell orders
    print("=== RECENT TXS (last 10) ===")
    slot = await conn._rpc("getSlot", [])
    print(f"  Current slot: {slot}")
    # Try to get some recent block transactions
    for s in range(max(0, slot - 5), slot + 1):
        try:
            blk = await conn._rpc("getBlock", [s])
            if blk and isinstance(blk, dict):
                txs = blk.get("transactions", [])
                if txs:
                    print(f"  Slot {s}: {len(txs)} txs")
                    for t in txs[:3]:
                        sig = t.get("signature", "?")[:16]
                        typ = t.get("type", "?")
                        st = t.get("status", "?")
                        print(f"    {sig}... type={typ} status={st}")
        except Exception:
            pass

    # 6. callContract to check lUSD balance_of
    reserve_pk = "CxLVBb5q31xJwAvnUzpgzJRdqFBnpDDVn9M2ZGyKLMw"
    print()
    print("=== callContract: lUSD.balance_of(reserve) ===")
    try:
        reserve_bytes = list(PublicKey.from_base58(reserve_pk).to_bytes())
        result = await conn._rpc("callContract", [{
            "program": lusd_addr,
            "function": "balance_of",
            "args": reserve_bytes,
            "from": reserve_pk,
        }])
        print(f"  Result: {json.dumps(result)[:500]}")
    except Exception as ex:
        print(f"  Error: {ex}")

if __name__ == "__main__":
    asyncio.run(main())
