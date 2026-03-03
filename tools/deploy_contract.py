#!/usr/bin/env python3
"""Deploy a single WASM contract to MoltChain validator.

Uses system program instruction type 17 (consensus deploy):
  data = [17 | code_length(4 LE) | code_bytes | init_data_json]
  accounts = [deployer, treasury]
"""

import sys
import os
import json
import struct
import asyncio
import hashlib
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from moltchain import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

RPC_URL = os.environ.get("CUSTODY_MOLT_RPC_URL", "http://127.0.0.1:8899")
KEYPAIR_DIR = Path(__file__).resolve().parent.parent / "keypairs"
DEPLOYER_PATH = KEYPAIR_DIR / "deployer.json"
SYSTEM_PROGRAM = PublicKey(b'\x00' * 32)


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


def find_treasury_keypair() -> str:
    """Discover treasury pubkey from genesis state."""
    repo_root = Path(__file__).resolve().parent.parent
    # Try common locations for treasury keypair
    candidates = list(repo_root.glob("data/*/genesis-keys/treasury-*.json"))
    for c in candidates:
        try:
            d = json.loads(c.read_text())
            pk = d.get("pubkey", d.get("address", ""))
            if pk:
                return pk
        except Exception:
            continue
    return ""


async def deploy(wasm_path: str, init_data: dict = None):
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

    # Resolve treasury pubkey
    treasury_b58 = os.environ.get("TREASURY_PUBKEY", "") or find_treasury_keypair()
    if not treasury_b58:
        print("\u274c Cannot resolve treasury pubkey. Set TREASURY_PUBKEY env or ensure data/*/genesis-keys/treasury-*.json exists.")
        sys.exit(1)
    treasury_pubkey = PublicKey(treasury_b58)
    print(f"\U0001f3e6 Treasury: {treasury_pubkey}")

    program_pubkey = derive_program_address(deployer.public_key(), wasm_bytes)
    print(f"\U0001f4cd Derived program address: {program_pubkey}")

    # Build deploy instruction data:
    # [17 | code_length(4 LE) | raw_wasm_bytes | optional_init_data_json]
    data = bytearray()
    data.append(17)  # instruction type: deploy
    data.extend(struct.pack('<I', len(wasm_bytes)))  # code length, little-endian u32
    data.extend(wasm_bytes)  # raw WASM bytecode
    if init_data:
        data.extend(json.dumps(init_data).encode('utf-8'))

    ix = Instruction(
        program_id=SYSTEM_PROGRAM,
        accounts=[deployer.public_key(), treasury_pubkey],
        data=bytes(data),
    )

    blockhash = await conn.get_recent_blockhash()
    tx = (
        TransactionBuilder()
        .add(ix)
        .set_recent_blockhash(blockhash)
        .build_and_sign(deployer)
    )

    print(f"\U0001f680 Sending deploy transaction ({len(data)} bytes)...")
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
        print("Usage: python deploy_contract.py <path/to/contract.wasm> [--init-data '{\"symbol\":\"...\"}']")
        sys.exit(1)
    init = None
    for i, a in enumerate(sys.argv):
        if a == "--init-data" and i + 1 < len(sys.argv):
            init = json.loads(sys.argv[i + 1])
    asyncio.run(deploy(sys.argv[1], init))
