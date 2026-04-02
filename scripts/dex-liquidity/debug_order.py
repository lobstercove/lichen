#!/usr/bin/env python3
"""Debug single order placement to find why orders aren't stored."""
import sys, json, struct, asyncio
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))
from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)

def load_keypair(path):
    return Keypair.load(Path(path))


async def main():
    conn = Connection("http://15.204.229.189:8899")
    kp = load_keypair(ROOT / "artifacts/testnet/genesis-keys/reserve_pool-lichen-testnet-1.json")
    print(f"Caller: {kp.address()}")
    
    # Build a simple sell order: 1 LICN at $0.10
    buf = bytearray(75)
    buf[0] = 0x02  # opcode: place_order
    pub_bytes = kp.address().to_bytes()
    buf[1:33] = pub_bytes[:32]
    struct.pack_into("<Q", buf, 33, 1)  # pair_id=1
    buf[41] = 1  # side=sell
    buf[42] = 0  # type=limit
    struct.pack_into("<Q", buf, 43, 100_000_000)  # price=0.1 * 1e9
    struct.pack_into("<Q", buf, 51, 1_000_000_000)  # qty=1 LICN (in spores)
    struct.pack_into("<Q", buf, 59, 0)  # expiry
    struct.pack_into("<Q", buf, 67, 0)  # trigger_price
    
    print(f"Order bytes ({len(buf)}): opcode={buf[0]} side={buf[41]} type={buf[42]}")
    print(f"  price={struct.unpack_from('<Q', buf, 43)[0]} qty={struct.unpack_from('<Q', buf, 51)[0]}")
    
    dex_addr = PublicKey.from_base58("6KLSjabyfRXE87VzTFGmWq9wHoY9izEoUfovPNeFmf9u")
    envelope = json.dumps({"Call": {"function": "call", "args": list(bytes(buf)), "value": 0}})
    data = envelope.encode("utf-8")
    
    print(f"Envelope size: {len(data)} bytes")
    
    ix = Instruction(CONTRACT_PROGRAM, [kp.address(), dex_addr], data)
    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(kp)
    
    print(f"Sending TX...")
    sig = await conn.send_transaction(tx)
    print(f"TX sig: {sig}")
    
    # Wait and get result
    for attempt in range(30):
        await asyncio.sleep(0.5)
        info = await conn.get_transaction(sig)
        if info:
            print(f"\n=== TX Result ===")
            for k in sorted(info.keys()):
                v = info[k]
                if isinstance(v, dict) and len(str(v)) > 200:
                    print(f"  {k}: <dict with {len(v)} keys>")
                elif isinstance(v, list) and len(str(v)) > 200:
                    print(f"  {k}: <list with {len(v)} items>")
                else:
                    print(f"  {k}: {v}")
            break
    else:
        print("TX not confirmed after 15s")

    # Check order count
    result = await conn._rpc("getProgramStorage", ["6KLSjabyfRXE87VzTFGmWq9wHoY9izEoUfovPNeFmf9u", {"limit": 5}])
    entries = result.get("entries", [])
    for e in entries:
        key = e.get("key_decoded") or e.get("key_hex", "")[:40]
        if "order_count" in str(key):
            print(f"\ndex_order_count: {e.get('value_hex', '')}")

asyncio.run(main())
