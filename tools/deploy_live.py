#!/usr/bin/env python3
"""Deploy MoltCoin, MoltPunks, and MoltSwap contracts to a live MoltChain validator."""

import sys
import os
import json
import struct
import asyncio
import hashlib
from pathlib import Path
from typing import Optional

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from moltchain import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

RPC_URL = "http://127.0.0.1:8899"
KEYPAIR_DIR = Path(__file__).resolve().parent.parent / "keypairs"
DEPLOYER_PATH = KEYPAIR_DIR / "deployer.json"
CONTRACT_PROGRAM = PublicKey(b'\xff' * 32)  # for Call instructions
SYSTEM_PROGRAM = PublicKey(b'\x00' * 32)   # for Deploy instructions (type 17)

CONTRACTS = [
    {"name": "MoltCoin",  "wasm": "moltcoin.wasm"},
    {"name": "MoltPunks", "wasm": "moltpunks.wasm"},
    {"name": "MoltSwap",  "wasm": "moltswap.wasm"},
]

WASM_SEARCH_DIRS = [
    Path(__file__).resolve().parent.parent / "contracts" / "target" / "wasm32-unknown-unknown" / "release",
    Path(__file__).resolve().parent.parent / "contracts" / "build",
    Path(__file__).resolve().parent.parent / "contracts",
]


def find_wasm(filename: str) -> Optional[Path]:
    for d in WASM_SEARCH_DIRS:
        p = d / filename
        if p.exists():
            return p
    return None


def load_or_create_deployer() -> Keypair:
    KEYPAIR_DIR.mkdir(parents=True, exist_ok=True)
    if DEPLOYER_PATH.exists():
        kp = Keypair.load(DEPLOYER_PATH)
        print(f"\U0001f511 Deployer: {kp.public_key()}")
        return kp
    kp = Keypair.generate()
    kp.save(DEPLOYER_PATH)
    print(f"\U0001f511 New deployer generated: {kp.public_key()}")
    return kp


def derive_program_address(deployer: PublicKey, wasm_bytes: bytes) -> PublicKey:
    h = hashlib.sha256(deployer.to_bytes() + wasm_bytes).digest()
    return PublicKey(h[:32])


async def deploy_contract(
    conn: Connection, deployer: Keypair, name: str, wasm_bytes: bytes,
    treasury_pubkey: PublicKey = None
) -> tuple:
    """Deploy a single contract via system program instruction type 17.
    Returns (signature, program_pubkey)."""
    program_pubkey = derive_program_address(deployer.public_key(), wasm_bytes)
    if treasury_pubkey is None:
        treasury_pubkey = deployer.public_key()
    data = bytearray()
    data.append(17)
    data.extend(struct.pack('<I', len(wasm_bytes)))
    data.extend(wasm_bytes)
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
    sig = await conn.send_transaction(tx)
    return sig, program_pubkey


async def call_contract(
    conn: Connection, caller: Keypair, program_pubkey: PublicKey,
    func: str, args: Optional[dict] = None
) -> str:
    """Send a Call instruction to a deployed contract. Returns signature."""
    args_bytes = json.dumps(args or {}).encode()
    payload = json.dumps({"Call": {"function": func, "args": list(args_bytes), "value": 0}})
    ix = Instruction(
        program_id=CONTRACT_PROGRAM,
        accounts=[caller.public_key(), program_pubkey],
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


async def main():
    import argparse
    parser = argparse.ArgumentParser(description="Deploy MoltChain core contracts")
    parser.add_argument("--rpc", default=RPC_URL, help="MoltChain RPC URL")
    parser.add_argument("--admin", default=None, help="Admin/treasury pubkey (base58). Required for mainnet.")
    parser.add_argument("--network", default="testnet", choices=["testnet", "mainnet"],
                        help="Network type. Mainnet requires --admin (multisig address).")
    args = parser.parse_args()

    deployer = load_or_create_deployer()
    conn = Connection(args.rpc)

    # Resolve admin pubkey — enforce multisig for mainnet
    if args.admin:
        admin_pubkey = PublicKey.from_base58(args.admin)
        if admin_pubkey == deployer.public_key() and args.network == "mainnet":
            print("❌ MAINNET ERROR: --admin must be a multisig address, not the deployer keypair")
            sys.exit(1)
        print(f"🏛️  Admin: {admin_pubkey}")
    else:
        if args.network == "mainnet":
            print("❌ MAINNET ERROR: --admin is required for mainnet deployments")
            print("   python3 deploy_live.py --network mainnet --admin <MULTISIG_PUBKEY>")
            sys.exit(1)
        admin_pubkey = deployer.public_key()
        print(f"⚠️  Admin (deployer — single-key, testnet only): {admin_pubkey}")

    # Check validator
    try:
        health = await conn.health()
        print(f"\u2705 Validator healthy: {health}")
    except Exception as e:
        print(f"\u274c Cannot reach validator at {RPC_URL}: {e}")
        sys.exit(1)

    deployed: dict = {}

    # --- Deploy phase ---
    print("\n\u2550\u2550\u2550 DEPLOYING CONTRACTS \u2550\u2550\u2550")
    for c in CONTRACTS:
        wasm_path = find_wasm(c["wasm"])
        if not wasm_path:
            print(f"\u26a0\ufe0f  {c['name']}: WASM not found ({c['wasm']}), skipping")
            continue
        wasm_bytes = wasm_path.read_bytes()
        print(f"\n\U0001f4e6 {c['name']} \u2014 {len(wasm_bytes)} bytes")
        try:
            sig, pubkey = await deploy_contract(conn, deployer, c["name"], wasm_bytes)
            deployed[c["name"]] = pubkey
            print(f"   \u2705 Deployed  sig={sig}")
            print(f"   \U0001f4cd Address   {pubkey}")
        except Exception as e:
            print(f"   \u274c Deploy failed: {e}")

    if not deployed:
        print("\n\u274c No contracts were deployed. Build WASM files first.")
        sys.exit(1)

    # --- Initialize phase ---
    print("\n\u2550\u2550\u2550 INITIALIZING CONTRACTS \u2550\u2550\u2550")
    for name, pubkey in deployed.items():
        try:
            sig = await call_contract(conn, deployer, pubkey, "initialize")
            print(f"   \u2705 {name}.initialize() \u2014 sig={sig}")
        except Exception as e:
            print(f"   \u26a0\ufe0f  {name}.initialize() failed: {e}")

    # --- Verify phase ---
    print("\n\u2550\u2550\u2550 VERIFYING CONTRACTS \u2550\u2550\u2550")
    for name, pubkey in deployed.items():
        try:
            info = await conn.get_contract_info(pubkey)
            print(f"   \u2705 {name} on-chain: {json.dumps(info)}")
        except Exception as e:
            print(f"   \u26a0\ufe0f  {name} verification failed: {e}")

    # --- Summary ---
    print("\n\u2550\u2550\u2550 DEPLOYMENT SUMMARY \u2550\u2550\u2550")
    for name, pubkey in deployed.items():
        print(f"   {name:12s} \u2192 {pubkey}")
    print(f"\n   Deployer: {deployer.public_key()}")
    print(f"   Contracts deployed: {len(deployed)}/{len(CONTRACTS)}")


if __name__ == "__main__":
    asyncio.run(main())
