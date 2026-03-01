#!/usr/bin/env python3
"""End-to-end tests for MoltChain marketplace contract operations."""

import sys
import os
import json
import asyncio
from pathlib import Path
from typing import Optional

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from moltchain import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

RPC_URL = "http://127.0.0.1:8899"
KEYPAIR_DIR = Path(__file__).resolve().parent.parent / "keypairs"
DEPLOYER_PATH = KEYPAIR_DIR / "deployer.json"
CONTRACT_PROGRAM = PublicKey(b'\xff' * 32)

passed = 0
failed = 0


def report(name: str, ok: bool, detail: str = ""):
    global passed, failed
    if ok:
        passed += 1
        print(f"  \u2705 {name}" + (f" \u2014 {detail}" if detail else ""))
    else:
        failed += 1
        print(f"  \u274c {name}" + (f" \u2014 {detail}" if detail else ""))


async def call_contract(
    conn: Connection, caller: Keypair, program: PublicKey,
    func: str, args: Optional[dict] = None
) -> str:
    args_bytes = json.dumps(args or {}).encode()
    payload = json.dumps({"Call": {"function": func, "args": list(args_bytes), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program],
        data=payload.encode(),
    )
    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(caller)
    )
    return await conn.send_transaction(tx)


async def find_marketplace(conn: Connection) -> Optional[PublicKey]:
    """Find a marketplace / MoltSwap contract from on-chain list."""
    try:
        result = await conn.get_all_contracts()
        contracts = result if isinstance(result, list) else result.get("contracts", [])
        for c in contracts:
            name = json.dumps(c).lower()
            if "swap" in name or "market" in name:
                cid = c.get("id") or c.get("contract_id") or c.get("address", "")
                if isinstance(cid, str) and len(cid) > 10:
                    return PublicKey(cid)
        # Fallback: use the last deployed contract
        if contracts:
            cid = (contracts[-1].get("id") or contracts[-1].get("contract_id")
                   or contracts[-1].get("address", ""))
            if isinstance(cid, str) and len(cid) > 10:
                return PublicKey(cid)
    except Exception:
        pass
    return None


async def main():
    conn = Connection(RPC_URL)

    try:
        await conn.health()
        print("\u2705 Validator reachable")
    except Exception as e:
        print(f"\u274c Cannot reach validator at {RPC_URL}: {e}")
        sys.exit(1)

    if not DEPLOYER_PATH.exists():
        print(f"\u274c Deployer keypair not found at {DEPLOYER_PATH}. Run deploy_live.py first.")
        sys.exit(1)
    deployer = Keypair.load(DEPLOYER_PATH)
    buyer = Keypair.generate()
    print(f"\U0001f511 Deployer: {deployer.public_key()}")
    print(f"\U0001f511 Buyer:    {buyer.public_key()}")

    marketplace = await find_marketplace(conn)
    if not marketplace:
        print("\u274c Marketplace contract not found on-chain. Deploy first.")
        sys.exit(1)
    print(f"\U0001f4cd Marketplace: {marketplace}\n")

    print("\u2550\u2550\u2550 Marketplace E2E Tests \u2550\u2550\u2550\n")

    # Test 1: list_nft
    try:
        sig = await call_contract(conn, deployer, marketplace, "list_nft",
                      {"token_id": 1, "price": 5000})
        report("list_nft", True, f"sig={sig}")
    except Exception as e:
        report("list_nft", False, str(e))

    # Test 2: buy_nft
    try:
        sig = await call_contract(conn, buyer, marketplace, "buy_nft",
                                  {"token_id": 1})
        report("buy_nft", True, f"sig={sig}")
    except Exception as e:
        report("buy_nft", False, str(e))

    # Test 3: cancel_listing
    try:
        sig = await call_contract(conn, deployer, marketplace, "cancel_listing",
                                  {"token_id": 2})
        report("cancel_listing", True, f"sig={sig}")
    except Exception as e:
        report("cancel_listing", False, str(e))

    total = passed + failed
    print(f"\n\u2550\u2550\u2550 Results: {passed}/{total} passed \u2550\u2550\u2550")
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    asyncio.run(main())
