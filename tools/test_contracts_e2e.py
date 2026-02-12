#!/usr/bin/env python3
"""End-to-end tests for MoltCoin contract on a live MoltChain validator."""

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


async def find_moltcoin(conn: Connection) -> Optional[PublicKey]:
    """Find MoltCoin contract from on-chain contract list."""
    try:
        result = await conn.get_all_contracts()
        contracts = result if isinstance(result, list) else result.get("contracts", [])
        for c in contracts:
            cid = c.get("id") or c.get("contract_id") or c.get("address", "")
            name = json.dumps(c).lower()
            if "moltcoin" in name or contracts.index(c) == 0:
                if isinstance(cid, str) and len(cid) > 10:
                    return PublicKey(cid)
    except Exception:
        pass
    return None


async def main():
    conn = Connection(RPC_URL)

    # Check validator
    try:
        await conn.health()
        print("\u2705 Validator reachable")
    except Exception as e:
        print(f"\u274c Cannot reach validator at {RPC_URL}: {e}")
        sys.exit(1)

    # Load deployer
    if not DEPLOYER_PATH.exists():
        print(f"\u274c Deployer keypair not found at {DEPLOYER_PATH}")
        print("   Run deploy_live.py first.")
        sys.exit(1)
    deployer = Keypair.load(DEPLOYER_PATH)
    print(f"\U0001f511 Deployer: {deployer.public_key()}")

    # Find MoltCoin
    contract = await find_moltcoin(conn)
    if not contract:
        print("\u274c MoltCoin contract not found on-chain. Deploy first.")
        sys.exit(1)
    print(f"\U0001f4cd MoltCoin contract: {contract}\n")

    print("\u2550\u2550\u2550 MoltCoin E2E Tests \u2550\u2550\u2550\n")

    # Test 1: initialize
    try:
        sig = await call_contract(conn, deployer, contract, "initialize")
        report("initialize", True, f"sig={sig}")
    except Exception as e:
        report("initialize", False, str(e))

    # Test 2: balance_of
    try:
        sig = await call_contract(conn, deployer, contract, "balance_of",
                                  {"account": str(deployer.public_key())})
        report("balance_of", True, f"sig={sig}")
    except Exception as e:
        report("balance_of", False, str(e))

    # Test 3: mint
    try:
        sig = await call_contract(conn, deployer, contract, "mint",
                                  {"to": str(deployer.public_key()), "amount": 1000})
        report("mint", True, f"sig={sig}")
    except Exception as e:
        report("mint", False, str(e))

    # Test 4: transfer
    recipient = Keypair.generate()
    try:
        sig = await call_contract(conn, deployer, contract, "transfer", {
            "from": str(deployer.public_key()),
            "to": str(recipient.public_key()),
            "amount": 100,
        })
        report("transfer", True, f"sig={sig}")
    except Exception as e:
        report("transfer", False, str(e))

    # Test 5: burn
    try:
        sig = await call_contract(conn, deployer, contract, "burn",
                                  {"from": str(deployer.public_key()), "amount": 50})
        report("burn", True, f"sig={sig}")
    except Exception as e:
        report("burn", False, str(e))

    # Summary
    total = passed + failed
    print(f"\n\u2550\u2550\u2550 Results: {passed}/{total} passed \u2550\u2550\u2550")
    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    asyncio.run(main())
