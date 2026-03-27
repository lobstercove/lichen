#!/usr/bin/env python3
"""Mint LICN tokens to reserve_pool and set DEX approval."""
import asyncio
import json
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
SPORES = 1_000_000_000


def load_kp(network, role):
    path = ROOT / "artifacts" / network / "genesis-keys"
    for f in path.glob("*.json"):
        if f.name.startswith(role):
            raw = json.loads(f.read_text())
            return Keypair.from_seed(bytes.fromhex(raw["secret_key"]))
    raise FileNotFoundError(f"No keypair for {role}")


async def resolve(conn, symbol):
    result = await conn._rpc("getAllSymbolRegistry", [100])
    for e in result.get("entries", []):
        if e.get("symbol") == symbol:
            return PublicKey.from_base58(e["program"])
    raise ValueError(f"Not found: {symbol}")


async def send_call(conn, signer, contract, fn, args_bytes):
    envelope = json.dumps({"Call": {"function": fn, "args": args_bytes, "value": 0}})
    ix = Instruction(CONTRACT_PROGRAM, [signer.public_key(), contract], envelope.encode("utf-8"))
    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    bh = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(bh)
    tx = tb.build_and_sign(signer)
    return await conn.send_transaction(tx)


async def get_token_balance(conn, token_addr, account_kp):
    storage = await conn._rpc("getProgramStorage", [str(token_addr)])
    hex_addr = account_kp.public_key().to_bytes().hex()
    key = f"licn_bal_{hex_addr}"
    for e in storage.get("entries", []):
        if e.get("key_decoded", "") == key:
            vh = e.get("value_hex", "0000000000000000")
            return int.from_bytes(bytes.fromhex(vh), "little")
    return 0


async def main():
    conn = Connection("http://15.204.229.189:8899")
    deployer = load_kp("testnet", "genesis-primary")
    reserve = load_kp("testnet", "reserve_pool")
    licn_addr = await resolve(conn, "LICN")
    dex_addr = await resolve(conn, "DEX")

    print(f"LICN token: {licn_addr}")
    print(f"DEX: {dex_addr}")
    print(f"Deployer: {deployer.public_key()}")
    print(f"Reserve: {reserve.public_key()}")

    bal = await get_token_balance(conn, licn_addr, reserve)
    print(f"Current LICN token balance: {bal / SPORES:,.4f}")

    # Mint 20M LICN tokens
    mint_amount = 20_000_000 * SPORES
    caller_b = list(deployer.public_key().to_bytes())
    to_b = list(reserve.public_key().to_bytes())
    amt_b = list(struct.pack("<Q", mint_amount))
    sig = await send_call(conn, deployer, licn_addr, "mint", caller_b + to_b + amt_b)
    print(f"Mint TX: {sig}")
    await asyncio.sleep(2)

    bal2 = await get_token_balance(conn, licn_addr, reserve)
    print(f"New LICN token balance: {bal2 / SPORES:,.4f}")

    # Approve DEX to spend 100M (generous overshoot)
    approve_amount = 100_000_000 * SPORES
    owner_b = list(reserve.public_key().to_bytes())
    spender_b = list(dex_addr.to_bytes())
    amt2_b = list(struct.pack("<Q", approve_amount))
    sig2 = await send_call(conn, reserve, licn_addr, "approve", owner_b + spender_b + amt2_b)
    print(f"Approve TX: {sig2}")
    await asyncio.sleep(1)
    print("Done!")


if __name__ == "__main__":
    asyncio.run(main())
