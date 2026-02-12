#!/usr/bin/env python3
"""Deploy a single WASM contract to MoltChain validator."""

import sys
import os
import json
import asyncio
import hashlib
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from moltchain import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

RPC_URL = os.environ.get("CUSTODY_MOLT_RPC_URL", "http://127.0.0.1:8899")
KEYPAIR_DIR = Path(__file__).resolve().parent.parent / "keypairs"
DEPLOYER_PATH = KEYPAIR_DIR / "deployer.json"
CONTRACT_PROGRAM = PublicKey(b'\xff' * 32)


def load_or_create_deployer() -> Keypair:
    """Load deployer keypair from file, or generate and save a new one."""
    KEYPAIR_DIR.mkdir(parents=True, exist_ok=True)
    if DEPLOYER_PATH.exists():
        kp = Keypair.load(DEPLOYER_PATH)
        print(f"\U0001f511 Loaded deployer: {kp.public_key()}")
        return kp
    kp = Keypair.generate()
    kp.save(DEPLOYER_PATH)
    print(f"\U0001f511 Generated new deployer: {kp.public_key()}")
    return kp


def derive_program_address(deployer: PublicKey, wasm_bytes: bytes) -> PublicKey:
    """SHA-256(deployer_bytes + wasm_bytes), first 32 bytes."""
    h = hashlib.sha256(deployer.to_bytes() + wasm_bytes).digest()
    return PublicKey(h[:32])


async def deploy(wasm_path: str):
    wasm_file = Path(wasm_path)
    if not wasm_file.exists():
        print(f"\u274c WASM file not found: {wasm_file}")
        print("   Build it first:  cd contracts/<name> && cargo build --release --target wasm32-unknown-unknown")
        sys.exit(1)

    wasm_bytes = wasm_file.read_bytes()
    print(f"\U0001f4e6 WASM loaded: {len(wasm_bytes)} bytes \u2014 {wasm_file.name}")

    deployer = load_or_create_deployer()
    conn = Connection(RPC_URL)

    # Verify validator is reachable
    try:
        await conn.health()
    except Exception as e:
        print(f"\u274c Cannot reach validator at {RPC_URL}: {e}")
        sys.exit(1)

    program_pubkey = derive_program_address(deployer.public_key(), wasm_bytes)
    print(f"\U0001f4cd Derived program address: {program_pubkey}")

    # Build deploy instruction
    payload = json.dumps({"Deploy": {"code": list(wasm_bytes), "init_data": []}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[deployer.public_key(), program_pubkey],
        data=payload.encode(),
    )

    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(deployer)
    )

    print("\U0001f680 Sending deploy transaction...")
    try:
        sig = await conn.send_transaction(tx)
        print(f"\u2705 Transaction sent \u2014 signature: {sig}")
    except Exception as e:
        print(f"\u274c Deploy transaction failed: {e}")
        sys.exit(1)

    # Verify deployment
    try:
        info = await conn.get_contract_info(program_pubkey)
        print(f"\u2705 Contract verified on-chain: {json.dumps(info, indent=2)}")
    except Exception as e:
        print(f"\u26a0\ufe0f  Contract info lookup failed (may need a block): {e}")

    print(f"\n\U0001f4cb Summary")
    print(f"   Program ID : {program_pubkey}")
    print(f"   Deployer   : {deployer.public_key()}")
    print(f"   Signature  : {sig}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python deploy_contract.py <path/to/contract.wasm>")
        sys.exit(1)
    asyncio.run(deploy(sys.argv[1]))
